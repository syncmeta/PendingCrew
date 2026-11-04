use super::{
    invalid,
    outcome::{self, Delivery, canonical_decision, core, denied, event},
    split,
};
use crate::{Result, Sender, Tx, asks, judgments, sessions, work};
use crewd_proto::{CoordinationAssignment, WorkAssignParams};
use crewd_store::{StoreError, meta_get, meta_set};
use crewd_work::{Event, EventKind, HowChanged, WorkId};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    receipt: CoordinationAssignment,
    intent: String,
    event: String,
    delivery: Delivery,
    seal: String,
}
impl Saved {
    fn seal(&self) -> Result<String> {
        judgments::fingerprint(&(&self.receipt, &self.intent, &self.event, &self.delivery))
    }
}

fn key(id: &str, decision: &str) -> String {
    format!("work.coordination.assignment.v1:{id}:{decision}")
}

pub(super) fn exists(c: &Connection, id: &str, decision: &str) -> Result<bool> {
    Ok(meta_get(c, &key(id, decision))?.is_some())
}

fn latest_key(id: &str) -> String {
    format!("work.coordination.assignment.latest.v1:{id}")
}

fn exact_next(before: &str, after: &str) -> bool {
    before.parse::<u64>().is_ok_and(|n| {
        n > 0
            && before == n.to_string()
            && n.checked_add(1)
                .is_some_and(|next| after == next.to_string())
    })
}

fn verify(receipt: &CoordinationAssignment, event: &Event, core: WorkId) -> Result<()> {
    let how = if receipt.captain == receipt.owner {
        HowChanged::Claim
    } else {
        HowChanged::Assign
    };
    if receipt.previous_owner == receipt.owner
        || !exact_next(&receipt.previous_revision, &receipt.revision)
        || event.at.0 != receipt.assigned_at_ms
        || event.by.session().map(|s| s.as_str()) != Some(receipt.captain.as_str())
        || !matches!(&event.kind, EventKind::OwnerChanged { id, from: Some(from), to, how: actual, .. }
            if *id == core && from.as_str() == receipt.previous_owner
                && to.as_str() == receipt.owner && *actual == how)
    {
        return Err(invalid());
    }
    Ok(())
}

fn read(c: &Connection, crew: &str, id: &str, decision: &str) -> Result<Option<Saved>> {
    if !canonical_decision(decision) {
        return Err(invalid());
    }
    let Some(raw) = meta_get(c, &key(id, decision))? else {
        return Ok(None);
    };
    let saved: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    let r = &saved.receipt;
    if r.crew_id != crew
        || r.id != id
        || r.decision_id != decision
        || !saved.delivery.valid()
        || saved.seal()? != saved.seal
    {
        return Err(invalid());
    }
    if split::exists(c, id, decision)? {
        return Err(invalid());
    }
    let event = event(c, crew, &r.event_seq)?;
    verify(r, &event, core(c, crew, id)?)?;
    if judgments::fingerprint(&event)? != saved.event {
        return Err(invalid());
    }
    Ok(Some(saved))
}

pub(super) fn latest(
    c: &Connection,
    crew: &str,
    id: &str,
) -> Result<Option<CoordinationAssignment>> {
    let Some(decision) = meta_get(c, &latest_key(id))? else {
        return Ok(None);
    };
    Ok(Some(
        read(c, crew, id, &decision)?.ok_or_else(invalid)?.receipt,
    ))
}

pub(super) fn lookup(
    c: &Connection,
    crew: &str,
    id: &str,
    decision: &str,
) -> Result<Option<CoordinationAssignment>> {
    if !canonical_decision(decision) {
        return Err(denied());
    }
    Ok(read(c, crew, id, decision)?.map(|saved| saved.receipt))
}

/// The caller must check its current agent connection, token and admission inside this transaction.
pub fn assign(
    tx: &mut Tx<'_>,
    captain: &str,
    decision: &str,
    p: WorkAssignParams,
) -> Result<CoordinationAssignment> {
    let (crew, core) = outcome::actor(tx.conn(), captain, &p.id, decision)?;
    let crew = &crew;
    if split::exists(tx.conn(), &p.id, decision)? {
        return Err(denied());
    }
    let intent = judgments::fingerprint(&(captain, &p.expected_revision, &p.owner, &p.note))?;
    if let Some(saved) = read(tx.conn(), crew, &p.id, decision)? {
        return if saved.intent == intent {
            Ok(saved.receipt)
        } else {
            Err(denied())
        };
    }
    let (previous_owner, delivery) = outcome::fresh(
        tx.conn(),
        crew,
        captain,
        &p.id,
        decision,
        &p.expected_revision,
    )?;
    if sessions::get(tx.conn(), &p.owner)?.crew_id != *crew {
        return Err(denied());
    }
    let book = asks::load_org(tx.conn())?;
    if book
        .item(core)
        .ok_or_else(invalid)?
        .pending_transfer
        .is_some()
    {
        return Err(denied());
    }
    let seq = book
        .state()
        .last_seq
        .checked_add(1)
        .ok_or_else(invalid)?
        .to_string();
    let previous_revision = p.expected_revision.clone();
    let owner = p.owner.clone();
    let plan = work::assign(tx, &Sender::Session(captain.into()), p)?;
    let event = event(tx.conn(), crew, &seq)?;
    let receipt = CoordinationAssignment {
        crew_id: crew.clone(),
        id: plan.id,
        decision_id: decision.into(),
        captain: captain.into(),
        previous_owner,
        owner,
        previous_revision,
        revision: plan.revision,
        event_seq: seq,
        assigned_at_ms: event.at.0,
    };
    verify(&receipt, &event, core)?;
    let mut saved = Saved {
        receipt,
        intent,
        event: judgments::fingerprint(&event)?,
        delivery,
        seal: String::new(),
    };
    saved.seal = saved.seal()?;
    meta_set(
        tx.conn(),
        &key(&saved.receipt.id, decision),
        &serde_json::to_string(&saved).map_err(StoreError::from)?,
    )?;
    meta_set(tx.conn(), &latest_key(&saved.receipt.id), decision)?;
    Ok(saved.receipt)
}
