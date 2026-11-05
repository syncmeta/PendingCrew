//! Risks and lessons are Core-owned; occurrence has a single same-crew edge.
use crate::{Result, StoreError, managed_objects, object_refs::ObjectKind};
use crewd_proto::{
    ResourceActor,
    managed_objects::{
        Lesson, LessonDraft, LessonSource, LessonStatus, Risk, RiskDraft, RiskOwner, RiskStatus,
    },
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const DDL: &str = r#"
ALTER TABLE managed_object_receipts ADD COLUMN member_confirmation INTEGER NOT NULL DEFAULT 0
    CHECK(member_confirmation IN (0,1));
CREATE TABLE managed_risks (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    body_json TEXT NOT NULL CHECK(json_valid(body_json) AND json_type(body_json)='object'),
    UNIQUE(number,crew_id)
);
CREATE INDEX managed_risks_session_review
    ON managed_risks(json_extract(body_json,'$.owner.sessionId'),
                     json_extract(body_json,'$.next_review_at'),number)
    WHERE json_extract(body_json,'$.owner.kind')='session'
      AND json_extract(body_json,'$.status') IN ('open','mitigated');
CREATE TABLE managed_lessons (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    body_json TEXT NOT NULL CHECK(json_valid(body_json) AND json_type(body_json)='object'),
    occurrence_risk INTEGER UNIQUE,
    FOREIGN KEY(occurrence_risk,crew_id) REFERENCES managed_risks(number,crew_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK(occurrence_risk IS NULL OR occurrence_risk < number)
);
"#;

fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}

pub fn risk_numbers(c: &Connection, before: Option<i64>, limit: u16) -> Result<Vec<i64>> {
    let max = before.map_or(i64::MAX, |n| n.saturating_sub(1));
    let mut q = c.prepare(
        "SELECT number FROM managed_risks WHERE number<=?1 ORDER BY number DESC LIMIT ?2",
    )?;
    Ok(q.query_map(params![max, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn lesson_numbers(c: &Connection, before: Option<i64>, limit: u16) -> Result<Vec<i64>> {
    let max = before.map_or(i64::MAX, |n| n.saturating_sub(1));
    let mut q = c.prepare(
        "SELECT number FROM managed_lessons WHERE number<=?1 ORDER BY number DESC LIMIT ?2",
    )?;
    Ok(q.query_map(params![max, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

#[derive(Serialize, Deserialize)]
struct RiskData {
    draft: RiskDraft,
    owner: RiskOwner,
    author: ResourceActor,
    updated_at: String,
    next_review_at: String,
    status: RiskStatus,
    status_reason: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LessonData {
    draft: LessonDraft,
    author: ResourceActor,
    last_checked_at: String,
    status: LessonStatus,
    stale_reason: Option<String>,
}

struct Stored {
    crew_id: String,
    source_id: String,
    created_at: String,
    revision: i64,
    frozen: bool,
    body: String,
    paired: Option<i64>,
}

fn projection(row: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    Ok(Stored {
        crew_id: row.get(0)?,
        source_id: row.get(1)?,
        created_at: row.get(2)?,
        revision: row.get(3)?,
        frozen: row.get(4)?,
        body: row.get(5)?,
        paired: row.get(6)?,
    })
}

pub fn risk(c: &Connection, number: i64) -> Result<Option<Risk>> {
    let entry = c.query_row(
        "SELECT r.crew_id,r.source_id,r.created_at,x.revision,c.archived_at IS NOT NULL,x.body_json,l.number
         FROM managed_risks x JOIN object_refs r ON x.number=r.number AND x.crew_id=r.crew_id AND r.kind='risk'
         JOIN crews c ON c.id=r.crew_id
         LEFT JOIN managed_lessons l ON l.occurrence_risk=x.number AND l.crew_id=x.crew_id
         WHERE x.number=?1",
        [number], projection,
    ).optional()?;
    entry
        .map(|entry| {
            let data: RiskData = serde_json::from_str(&entry.body)?;
            if !managed_objects::source(ObjectKind::Risk, &entry.source_id)
                || !crate::managed_decisions::actor_valid(&data.author)
                || (data.status == RiskStatus::Occurred && entry.paired.is_none())
                || (entry.paired.is_some()
                    && !matches!(data.status, RiskStatus::Occurred | RiskStatus::Closed))
                || (matches!(data.status, RiskStatus::Mitigated | RiskStatus::Closed)
                    && data.status_reason.is_none())
            {
                return Err(invalid("invalid managed risk identity or occurrence"));
            }
            Ok(Risk {
                id: format!("R{number}"),
                crew_id: entry.crew_id,
                revision: entry.revision.to_string(),
                draft: data.draft,
                owner: data.owner,
                author: data.author,
                created_at: entry.created_at,
                updated_at: data.updated_at,
                next_review_at: data.next_review_at,
                status: data.status,
                status_reason: data.status_reason,
                became_lesson_id: entry.paired.map(|n| format!("L{n}")),
                frozen: entry.frozen,
            })
        })
        .transpose()
}

/// Passive context read only. A removed/reassigned session or a terminal
/// risk cannot produce a hint, and this query never schedules a wake.
pub fn due_owned_risks(c: &Connection, session: &str, now_ms: i64) -> Result<Vec<Risk>> {
    let due = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
        .ok_or_else(|| invalid("managed risk context date out of range"))?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut q = c.prepare(
        "SELECT x.number FROM managed_risks x
         JOIN sessions s ON s.id=?1 AND s.crew_id=x.crew_id AND s.role<>'admin'
         JOIN crews c ON c.id=x.crew_id AND c.archived_at IS NULL
         WHERE json_extract(x.body_json,'$.owner.kind')='session'
           AND json_extract(x.body_json,'$.status') IN ('open','mitigated')
           AND json_extract(x.body_json,'$.owner.sessionId')=?1
           AND json_extract(x.body_json,'$.next_review_at')<=?2
         ORDER BY json_extract(x.body_json,'$.next_review_at'),x.number LIMIT 1",
    )?;
    let ids = q
        .query_map(params![session, due], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    ids.into_iter()
        .map(|id| risk(c, id)?.ok_or_else(|| invalid("due risk missing")))
        .collect()
}

pub fn lesson(c: &Connection, number: i64) -> Result<Option<Lesson>> {
    let entry = c.query_row(
        "SELECT r.crew_id,r.source_id,r.created_at,x.revision,c.archived_at IS NOT NULL,x.body_json,x.occurrence_risk
         FROM managed_lessons x JOIN object_refs r ON x.number=r.number AND x.crew_id=r.crew_id AND r.kind='lesson'
         JOIN crews c ON c.id=r.crew_id WHERE x.number=?1",
        [number], projection,
    ).optional()?;
    entry
        .map(|entry| {
            let data: LessonData = serde_json::from_str(&entry.body)?;
            let risk_id = entry.paired.map(|n| format!("R{n}"));
            let mut sources = data
                .draft
                .learned_from
                .iter()
                .filter_map(|source| match source {
                    LessonSource::Risk { id } => Some(id),
                    _ => None,
                });
            if !managed_objects::source(ObjectKind::Lesson, &entry.source_id)
                || !crate::managed_decisions::actor_valid(&data.author)
                || sources.next() != risk_id.as_ref()
                || sources.next().is_some()
                || (data.status == LessonStatus::Stale) != data.stale_reason.is_some()
            {
                return Err(invalid(
                    "invalid managed lesson identity, state or occurrence",
                ));
            }
            Ok(Lesson {
                id: format!("L{number}"),
                crew_id: entry.crew_id,
                revision: entry.revision.to_string(),
                draft: data.draft,
                author: data.author,
                created_at: entry.created_at,
                last_checked_at: data.last_checked_at,
                status: data.status,
                stale_reason: data.stale_reason,
                risk_id,
                frozen: entry.frozen,
            })
        })
        .transpose()
}

pub fn record_risk(
    c: &Connection,
    crew: &str,
    draft: RiskDraft,
    owner: RiskOwner,
    author: ResourceActor,
    now: &str,
    next_review_at: String,
) -> Result<Risk> {
    managed_objects::transaction(c)?;
    let reference = managed_objects::allocate(c, ObjectKind::Risk, crew, now)?;
    let data = RiskData {
        draft,
        owner,
        author,
        updated_at: now.into(),
        next_review_at,
        status: RiskStatus::Open,
        status_reason: None,
    };
    c.execute(
        "INSERT INTO managed_risks(number,crew_id,revision,body_json) VALUES(?1,?2,1,?3)",
        params![reference.number, crew, serde_json::to_string(&data)?],
    )?;
    risk(c, reference.number)?.ok_or_else(|| invalid("recorded managed risk missing"))
}

pub fn save_risk(c: &Connection, number: i64, expected: i64, risk: Risk) -> Result<Risk> {
    managed_objects::transaction(c)?;
    let revision = expected
        .checked_add(1)
        .ok_or_else(|| invalid("managed risk revision exhausted"))?;
    let data = RiskData {
        draft: risk.draft,
        owner: risk.owner,
        author: risk.author,
        updated_at: risk.updated_at,
        next_review_at: risk.next_review_at,
        status: risk.status,
        status_reason: risk.status_reason,
    };
    let changed = c.execute(
        "UPDATE managed_risks SET revision=?3,body_json=?4 WHERE number=?1 AND revision=?2",
        params![number, expected, revision, serde_json::to_string(&data)?],
    )?;
    if changed != 1 {
        return Err(invalid("managed risk changed while writing"));
    }
    self::risk(c, number)?.ok_or_else(|| invalid("updated managed risk missing"))
}

pub fn record_lesson(
    c: &Connection,
    crew: &str,
    draft: LessonDraft,
    author: ResourceActor,
    now: &str,
    occurrence_risk: Option<i64>,
) -> Result<Lesson> {
    managed_objects::transaction(c)?;
    let reference = managed_objects::allocate(c, ObjectKind::Lesson, crew, now)?;
    let data = LessonData {
        draft,
        author,
        last_checked_at: now.into(),
        status: LessonStatus::Active,
        stale_reason: None,
    };
    c.execute(
        "INSERT INTO managed_lessons(number,crew_id,revision,body_json,occurrence_risk) VALUES(?1,?2,1,?3,?4)",
        params![reference.number, crew, serde_json::to_string(&data)?, occurrence_risk],
    )?;
    let lesson =
        lesson(c, reference.number)?.ok_or_else(|| invalid("recorded managed lesson missing"))?;
    crate::managed_artifacts::capture_lesson(c, &lesson, now)?;
    Ok(lesson)
}

pub fn save_lesson(c: &Connection, number: i64, expected: i64, lesson: Lesson) -> Result<Lesson> {
    managed_objects::transaction(c)?;
    let revision = expected
        .checked_add(1)
        .ok_or_else(|| invalid("managed lesson revision exhausted"))?;
    let data = LessonData {
        draft: lesson.draft,
        author: lesson.author,
        last_checked_at: lesson.last_checked_at,
        status: lesson.status,
        stale_reason: lesson.stale_reason,
    };
    let changed = c.execute(
        "UPDATE managed_lessons SET revision=?3,body_json=?4 WHERE number=?1 AND revision=?2",
        params![number, expected, revision, serde_json::to_string(&data)?],
    )?;
    if changed != 1 {
        return Err(invalid("managed lesson changed while writing"));
    }
    self::lesson(c, number)?.ok_or_else(|| invalid("updated managed lesson missing"))
}
