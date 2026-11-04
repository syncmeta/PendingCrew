use super::{
    assignment, invalid,
    outcome::{self, Delivery, canonical_decision, core, denied, event},
};
use crate::{Result, Sender, Tx, asks, judgments, sessions, work};
use crewd_proto::{CoordinationSplit, CoordinationSplitChild};
use crewd_store::{StoreError, meta_get, meta_set, object_refs::ObjectKind};
use crewd_work::{Body, CrewId, Event, EventKind, ItemRef, NewItem, SessionId, Status, WorkId};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const TOOL: &str = "work_item_coordination_split";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SplitParams {
    pub id: String,
    pub expected_revision: String,
    pub coordination_decision_id: String,
    pub children: Vec<SplitChild>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SplitChild {
    pub title: String,
    pub goal: String,
    pub acceptance: String,
    #[serde(deserialize_with = "explicit_owner")]
    pub owner: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

fn explicit_owner<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    receipt: CoordinationSplit,
    intent: String,
    events: Vec<String>,
    delivery: Delivery,
    seal: String,
}
impl Saved {
    fn seal(&self) -> Result<String> {
        judgments::fingerprint(&(&self.receipt, &self.intent, &self.events, &self.delivery))
    }
}

fn key(id: &str, decision: &str) -> String {
    format!("work.coordination.split.v1:{id}:{decision}")
}

fn latest_key(id: &str) -> String {
    format!("work.coordination.split.latest.v1:{id}")
}

pub(super) fn exists(c: &Connection, id: &str, decision: &str) -> Result<bool> {
    Ok(meta_get(c, &key(id, decision))?.is_some())
}

fn matches_child(
    r: &CoordinationSplit,
    child: &CoordinationSplitChild,
    e: &Event,
    parent: WorkId,
    id: WorkId,
) -> bool {
    child.revision == "1"
        && e.at.0 == r.split_at_ms
        && e.by.session().map(|s| s.as_str()) == Some(r.captain.as_str())
        && matches!(&e.kind, EventKind::ItemCreated { id: found, parent: Some(p), sources, owner, .. }
            if *found == id && *p == parent && sources.is_empty()
                && owner.as_ref().map(|s| s.as_str()) == child.owner.as_deref())
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
        || !r
            .parent_revision
            .parse::<u64>()
            .is_ok_and(|n| n > 0 && n.to_string() == r.parent_revision)
        || !(2..=8).contains(&r.children.len())
        || r.children.len() != saved.events.len()
        || !saved.delivery.valid()
        || saved.seal()? != saved.seal
        || assignment::exists(c, id, decision)?
    {
        return Err(invalid());
    }
    let parent = core(c, crew, id)?;
    let mut prev_seq: Option<u64> = None;
    let mut ids = BTreeSet::new();
    for (child, hash) in r.children.iter().zip(&saved.events) {
        let event = event(c, crew, &child.event_seq)?;
        let child_core = core(c, crew, &child.id)?;
        if child_core == parent
            || !ids.insert(child_core)
            || prev_seq.is_some_and(|prev| prev.checked_add(1) != Some(event.seq))
            || !matches_child(r, child, &event, parent, child_core)
            || judgments::fingerprint(&event)? != *hash
        {
            return Err(invalid());
        }
        prev_seq = Some(event.seq);
    }
    Ok(Some(saved))
}

pub(super) fn latest(c: &Connection, crew: &str, id: &str) -> Result<Option<CoordinationSplit>> {
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
) -> Result<Option<CoordinationSplit>> {
    if !canonical_decision(decision) {
        return Err(denied());
    }
    Ok(read(c, crew, id, decision)?.map(|saved| saved.receipt))
}

fn child_item(c: &Connection, crew: &str, parent: WorkId, child: &SplitChild) -> Result<NewItem> {
    if [
        (&child.title, 200),
        (&child.goal, 2000),
        (&child.acceptance, 2000),
    ]
    .into_iter()
    .any(|(value, max)| value.trim().is_empty() || value.chars().nth(max).is_some())
        || child.depends_on.len() > 16
    {
        return Err(denied());
    }
    let owner = child
        .owner
        .as_ref()
        .map(|id| {
            if sessions::get(c, id)?.crew_id != crew {
                return Err(denied());
            }
            Ok(SessionId::new(id))
        })
        .transpose()?;
    let mut seen = BTreeSet::new();
    let mut depends_on = Vec::with_capacity(child.depends_on.len());
    for id in &child.depends_on {
        let dependency = core(c, crew, id)?;
        if dependency == parent || !seen.insert(dependency) {
            return Err(denied());
        }
        depends_on.push(ItemRef::Id(dependency));
    }
    Ok(NewItem {
        crew_id: Some(CrewId::new(crew)),
        title: child.title.clone(),
        body: Body {
            goal: child.goal.clone(),
            acceptance: child.acceptance.clone(),
            ..Body::default()
        },
        owner,
        depends_on,
        ..NewItem::default()
    })
}

/// The caller must check current agent connection, token and admission in this transaction.
pub fn apply(tx: &mut Tx<'_>, captain: &str, p: SplitParams) -> Result<CoordinationSplit> {
    let decision = &p.coordination_decision_id;
    let (crew, parent) = outcome::actor(tx.conn(), captain, &p.id, decision)?;
    if assignment::exists(tx.conn(), &p.id, decision)? || !(2..=8).contains(&p.children.len()) {
        return Err(denied());
    }
    let intent = judgments::fingerprint(&(captain, &p))?;
    if let Some(saved) = read(tx.conn(), &crew, &p.id, decision)? {
        return if saved.intent == intent {
            Ok(saved.receipt)
        } else {
            Err(denied())
        };
    }
    let (_, delivery) = outcome::fresh(
        tx.conn(),
        &crew,
        captain,
        &p.id,
        decision,
        &p.expected_revision,
    )?;
    let book = asks::load_org(tx.conn())?;
    let previous_parent = book.item(parent).ok_or_else(invalid)?;
    let children = p
        .children
        .iter()
        .map(|c| child_item(tx.conn(), &crew, parent, c))
        .collect::<Result<Vec<_>>>()?;
    let by = Sender::Session(captain.into());
    let actor = work::actor(tx.conn(), &by, &crew)?;
    let created = asks::with_book(tx, &crew, |b, now| {
        b.create_coordination_children(&actor, now, parent, children)
            .map(|applied| applied.value)
    })
    .map_err(work::command_refusal)?;
    let after = asks::load_org(tx.conn())?;
    if after.item(parent) != Some(previous_parent) || created.len() != p.children.len() {
        return Err(invalid());
    }
    let mut receipt = CoordinationSplit {
        crew_id: crew.clone(),
        id: p.id,
        decision_id: decision.clone(),
        captain: captain.into(),
        parent_revision: p.expected_revision,
        split_at_ms: 0,
        children: Vec::with_capacity(created.len()),
    };
    let mut events = Vec::with_capacity(created.len());
    let mut seq = book.state().last_seq;
    for (id, intent) in created.into_iter().zip(&p.children) {
        seq = seq.checked_add(1).ok_or_else(invalid)?;
        let event = event(tx.conn(), &crew, &seq.to_string())?;
        let reference = crewd_store::work_organization::reference_for_core(
            tx.conn(),
            ObjectKind::Plan,
            &id.to_string(),
        )?
        .filter(|r| r.crew_id == crew)
        .ok_or_else(invalid)?;
        let state = after.item(id).ok_or_else(invalid)?;
        let child = CoordinationSplitChild {
            id: reference.id(),
            owner: intent.owner.clone(),
            revision: state.revision.to_string(),
            event_seq: seq.to_string(),
        };
        if receipt.children.is_empty() {
            receipt.split_at_ms = event.at.0;
        }
        if state.status != Status::Open
            || state.body.level != previous_parent.body.level
            || !matches_child(&receipt, &child, &event, parent, id)
        {
            return Err(invalid());
        }
        work::changed(tx, &by, &crew, &child.id)?;
        events.push(judgments::fingerprint(&event)?);
        receipt.children.push(child);
    }
    if after.state().last_seq != seq {
        return Err(invalid());
    }
    work::changed(tx, &by, &crew, &receipt.id)?;
    let mut saved = Saved {
        receipt,
        intent,
        events,
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
