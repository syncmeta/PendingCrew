//! Shared authorization and durable input evidence for coordination effects.
use super::{invalid, matches_wake, paused, record, snapshot};
use crate::{CollabError, Result, crews, sessions, wakes, work};
use crewd_proto::{CoordinationPhase, Role};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind},
};
use crewd_work::{Event, WorkId};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Delivery {
    wake_id: i64,
    attempts: u32,
    settled_at: String,
    decision_fingerprint: String,
}
impl Delivery {
    pub(super) fn valid(&self) -> bool {
        self.wake_id > 0
            && self.attempts > 0
            && self.decision_fingerprint.len() == 64
            && self
                .decision_fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && chrono::DateTime::parse_from_rfc3339(&self.settled_at)
                .is_ok_and(|at| self.settled_at == crewd_store::ts_from_ms(at.timestamp_millis()))
    }
}

pub(super) fn denied() -> CollabError {
    use crate::refusal::{Code, Refusal};
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}

pub(super) fn canonical_decision(decision: &str) -> bool {
    uuid::Uuid::parse_str(decision).is_ok_and(|uuid| uuid.to_string() == decision)
}

pub(super) fn core(c: &Connection, crew: &str, id: &str) -> Result<WorkId> {
    let reference = object_refs::get(c, ObjectKind::Plan, work::number(id)?)?
        .filter(|r| r.crew_id == crew)
        .ok_or_else(denied)?;
    work::core_plan(c, &reference)
}

pub(super) fn event(c: &Connection, crew: &str, seq: &str) -> Result<Event> {
    let seq: i64 = seq
        .parse()
        .ok()
        .filter(|n: &i64| *n > 0 && n.to_string() == seq)
        .ok_or_else(invalid)?;
    let raw: String = c
        .query_row(
            "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
            (seq, crew),
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::from)?
        .ok_or_else(invalid)?;
    let event: Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if event.seq != seq as u64 {
        return Err(invalid());
    }
    Ok(event)
}

/// Check current captain identity and exact same-crew work for both replay and mutation.
pub(super) fn actor(
    c: &Connection,
    captain: &str,
    id: &str,
    decision: &str,
) -> Result<(String, WorkId)> {
    if !canonical_decision(decision) {
        return Err(denied());
    }
    let crew = sessions::get(c, captain)?.crew_id;
    crews::refuse_archived(c, &crew)?;
    if sessions::role_of(c, captain)? != Role::Captain {
        return Err(denied());
    }
    let core = core(c, &crew, id)?;
    Ok((crew, core))
}

/// Recheck live facts, restore/Stop, revision and actual original delivery before any new effect.
pub(super) fn fresh(
    c: &Connection,
    crew: &str,
    captain: &str,
    id: &str,
    decision: &str,
    revision: &str,
) -> Result<(String, Delivery)> {
    let r = record(c, id)?.ok_or_else(denied)?;
    let current = work::get(c, id)?;
    if r.snapshot.crew != crew
        || r.snapshot.captain != captain
        || r.view.recipient.as_deref() != Some(captain)
        || r.view.decision_id != decision
        || !r.view.adopted
        || !matches!(r.view.phase, CoordinationPhase::Settled)
        || current.revision != revision
        || paused(c, &r.snapshot)?
        || snapshot(c, id)?.is_none_or(|s| s.fingerprint != r.snapshot.fingerprint)
    {
        return Err(denied());
    }
    let wake_id = r.view.wake_id.ok_or_else(denied)?;
    let wake = wakes::by_ids(c, &[wake_id])?
        .into_iter()
        .next()
        .filter(|wake| matches_wake(&r, wake) && wake.state == wakes::WakeState::Delivered)
        .ok_or_else(denied)?;
    let delivery = Delivery {
        wake_id,
        attempts: wake.attempts,
        settled_at: wake.settled_at.ok_or_else(denied)?,
        decision_fingerprint: r.snapshot.fingerprint,
    };
    if !delivery.valid() {
        return Err(denied());
    }
    Ok((r.snapshot.owner, delivery))
}
