//! Source-authorized annotations and real typed targets, never copied business state.
use crate::{
    Change, CollabError, ProgressKind, Result, Sender, Tx, crews,
    managed_objects::{self as objects, Actor},
    refusal::{Code, Refusal},
    resource_acl, sessions,
};
use crewd_proto::{
    managed_artifacts::Subject,
    managed_objects::{Object, Scope},
    managed_tables_v2::*,
};
use crewd_store::{
    StoreError, managed_artifacts as artifacts, managed_links as db, managed_tables_v2 as tables,
    object_refs::{self, ObjectKind},
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[path = "managed_link_projection.rs"]
mod projection;
pub(crate) use projection::Projection;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveMode {
    Read,
    FreshWrite,
}
pub trait ExternalTargets {
    /// Availability only; every actual read/write still checks current authority.
    fn supports_knowledge(&self) -> bool {
        false
    }
    fn resolve(
        &self,
        c: &Connection,
        actor: Actor<'_>,
        target: &TargetRef,
        mode: ResolveMode,
    ) -> Result<ResolvedTarget>;
}
pub struct DenyExternal;
impl ExternalTargets for DenyExternal {
    fn resolve(
        &self,
        _: &Connection,
        _: Actor<'_>,
        _: &TargetRef,
        _: ResolveMode,
    ) -> Result<ResolvedTarget> {
        Err(invalid("unsupportedKnowledgeTarget"))
    }
}
pub(crate) fn invalid(f: &str) -> CollabError {
    resource_acl::invalid(f)
}
pub(crate) fn forbidden() -> CollabError {
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}
pub(crate) fn missing(id: &str) -> CollabError {
    CollabError::NotFound(Refusal::new(Code::ResourceNotFound).arg("id", id))
}
pub(crate) fn conflict(id: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ResourceRequestConflict).arg("request", id))
}
pub(crate) fn changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedLinksChanged))
}
pub(crate) fn sender(a: Actor<'_>) -> Sender {
    match a {
        Actor::Human(_) => Sender::Human,
        Actor::Session(s) => Sender::Session(s.into()),
    }
}
pub(crate) fn author(a: Actor<'_>) -> Author {
    match a {
        Actor::Human(_) => Author::Human {},
        Actor::Session(s) => Author::Session {
            session_id: s.into(),
        },
    }
}
pub(crate) fn num(s: &str, p: char) -> Result<i64> {
    tables::number(s, p).map_err(|_| invalid("id"))
}
fn reference(
    c: &Connection,
    id: &str,
    p: char,
    k: ObjectKind,
) -> Result<object_refs::ObjectReference> {
    object_refs::get(c, k, num(id, p)?)?.ok_or_else(|| missing(id))
}
fn channel_read(c: &Connection, a: Actor<'_>, id: &str) -> Result<()> {
    if let Actor::Session(s) = a
        && !crate::channels::readable_by(c, id, s)?
    {
        return Err(forbidden());
    }
    Ok(())
}
pub fn resolve(
    c: &Connection,
    a: Actor<'_>,
    t: &TargetRef,
    x: &dyn ExternalTargets,
) -> Result<ResolvedTarget> {
    Projection::new(c, a, x).resolve(t, ResolveMode::Read)
}
fn resolve_uncached(
    c: &Connection,
    a: Actor<'_>,
    t: &TargetRef,
    x: &dyn ExternalTargets,
    mode: ResolveMode,
) -> Result<ResolvedTarget> {
    a.principal(c)?;
    let (summary, owner) = match t {
        TargetRef::Work { id } => {
            let w = crate::work::get(c, id)?;
            (w.title, Some(w.crew_id))
        }
        TargetRef::Requirement { id } => {
            let w = crate::requirements::get(c, id)?;
            (w.todo.text, Some(w.todo.crew_id))
        }
        TargetRef::Ask { id } => {
            let r = reference(c, id, 'H', ObjectKind::HumanTask)?;
            resource_acl::actor(c, &sender(a), &r.crew_id, false)?;
            let h = crate::asks::get(c, &r.crew_id, id)?;
            (h.question, Some(r.crew_id))
        }
        TargetRef::Table { id } => {
            let t = tables::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id))?;
            (t.title, Some(t.crew_id))
        }
        TargetRef::Row { id } => {
            let (t, _, _, cells) =
                tables::row_base(c, num(id, 'O')?)?.ok_or_else(|| missing(id))?;
            let title = cells
                .iter()
                .find(|v| v.field_id == t.title_field_id)
                .map(|v| match &v.value {
                    Cell::Text(t) | Cell::Integer(t) => t.clone(),
                    Cell::Real(n) => n.to_string(),
                    Cell::Null => id.clone(),
                })
                .unwrap_or_else(|| id.clone());
            (title, Some(t.crew_id))
        }
        TargetRef::Decision { id } | TargetRef::Risk { id } | TargetRef::Lesson { id } => {
            let o = objects::get_with_defaults(c, a, id, true)?;
            match o {
                Object::Decision { decision: d } => (d.draft.question, Some(d.crew_id)),
                Object::Risk { risk: r } => (r.draft.summary, Some(r.crew_id)),
                Object::Lesson { lesson: l } => (l.draft.lesson, Some(l.crew_id)),
                _ => return Err(invalid("target")),
            }
        }
        TargetRef::Artifact { reference } => {
            let p = artifacts::get(c, reference)?.ok_or_else(|| missing("artifact"))?;
            (p.reference.key, None)
        }
        TargetRef::Crew { id } => {
            let crew = crews::get(c, id)?;
            (crew.title, Some(crew.id))
        }
        TargetRef::Session { id } => {
            let s = sessions::get(c, id)?;
            (
                s.title.or(s.display_name).unwrap_or_else(|| id.clone()),
                Some(s.crew_id),
            )
        }
        TargetRef::Channel { id } => {
            channel_read(c, a, id)?;
            let ch = crate::channels::get(c, id)?;
            (ch.name, Some(ch.crew_id))
        }
        TargetRef::Message { id } => {
            let m = crewd_store::messages::get(c, id)?.ok_or_else(|| missing(id))?;
            channel_read(c, a, &m.channel_id)?;
            let ch = crate::channels::get(c, &m.channel_id)?;
            (m.text, Some(ch.crew_id))
        }
        TargetRef::Human {} => ("Human".into(), None),
        TargetRef::Document { .. } | TargetRef::Knowledge { .. } => {
            return x.resolve(c, a, t, mode);
        }
    };
    let message_channel_id = if let TargetRef::Message { id } = t {
        Some(
            crewd_store::messages::get(c, id)?
                .ok_or_else(|| missing(id))?
                .channel_id,
        )
    } else {
        None
    };
    Ok(ResolvedTarget {
        message_channel_id,
        target: t.clone(),
        summary: summary.chars().take(280).collect(),
        owner_crew_id: owner,
    })
}
pub(crate) fn source_target(s: &SourceRef) -> TargetRef {
    match s {
        SourceRef::Work { id } => TargetRef::Work { id: id.clone() },
        SourceRef::Requirement { id } => TargetRef::Requirement { id: id.clone() },
        SourceRef::Ask { id } => TargetRef::Ask { id: id.clone() },
        SourceRef::Table { id } => TargetRef::Table { id: id.clone() },
        SourceRef::Row { id } => TargetRef::Row { id: id.clone() },
        SourceRef::Decision { id } => TargetRef::Decision { id: id.clone() },
        SourceRef::Risk { id } => TargetRef::Risk { id: id.clone() },
        SourceRef::Lesson { id } => TargetRef::Lesson { id: id.clone() },
        SourceRef::Artifact { reference, .. } => TargetRef::Artifact {
            reference: reference.clone(),
        },
    }
}
pub(crate) fn may_author(c: &Connection, a: Actor<'_>, crew: &str, owner: &Author) -> Result<bool> {
    Ok(match a {
        Actor::Human(_) => true,
        Actor::Session(s) => {
            matches!(owner,Author::Session{session_id} if session_id==s)
                || crews::acts_as_captain_of(c, s, crew)?
        }
    })
}
pub(crate) fn source(
    c: &Connection,
    a: Actor<'_>,
    s: &SourceRef,
    write: bool,
    x: &dyn ExternalTargets,
) -> Result<String> {
    Projection::new(c, a, x).source(s, write)
}
pub(crate) fn finish(tx: &mut Tx<'_>, a: Actor<'_>, crew: &str) -> Result<()> {
    if let Actor::Session(s) = a {
        sessions::note_progress(tx, s, ProgressKind::Ontology)?;
    }
    tx.note(Change::ManagedLinksChanged {
        crew_id: crew.into(),
    });
    Ok(())
}
pub(crate) fn request(
    c: &Connection,
    a: Actor<'_>,
    scope: &Scope,
    crew: &str,
    uuid: &str,
    write: bool,
) -> Result<String> {
    if !crewd_proto::valid_remote_uuid(uuid)
        || !crewd_proto::valid_remote_uuid(&scope.restore_epoch)
    {
        return Err(invalid("requestId/scope"));
    }
    let p = a.principal(c)?;
    resource_acl::actor(c, &sender(a), crew, write)?;
    if objects::scope(c)?.db_id != scope.db_id {
        return Err(forbidden());
    }
    Ok(p)
}
pub(crate) fn declare(
    c: &Connection,
    a: Actor<'_>,
    crew: &str,
    v: &ManualTargetWrite,
    x: &dyn ExternalTargets,
    now: &str,
) -> Result<TargetRef> {
    Projection::new(c, a, x).declare(crew, v, now)
}
pub fn submit(
    tx: &mut Tx<'_>,
    a: Actor<'_>,
    r: ManualSubmit,
    x: &dyn ExternalTargets,
) -> Result<ManualReceipt> {
    let c = tx.conn();
    let p = request(c, a, &r.scope, &r.crew_id, &r.request_id, false)?;
    let digest = db::intent(
        LINK_SUBMIT,
        &r.scope,
        &r.crew_id,
        &r.request_id,
        &r.operation,
    )?;
    if let Some(saved) = db::receipt(c, &r.crew_id, &p, &r.request_id)? {
        return match saved {
            db::ReadReceipt::Active(s) if s.method == LINK_SUBMIT && s.digest == digest => {
                Ok(serde_json::from_value(s.result)?)
            }
            _ => Err(conflict(&r.request_id)),
        };
    }
    if objects::scope(c)? != r.scope {
        return Err(changed());
    }
    resource_acl::actor(c, &sender(a), &r.crew_id, true)?;
    let (s, expected) = match &r.operation {
        ManualOperation::Link {
            source,
            expected_manual_link_revision,
            ..
        }
        | ManualOperation::Unlink {
            source,
            expected_manual_link_revision,
            ..
        } => (source, expected_manual_link_revision),
    };
    let owner = source(c, a, s, true, x)?;
    if owner != r.crew_id {
        return Err(forbidden());
    }
    let before = db::stamp(c, s, &owner)?;
    if before.to_string() != *expected {
        return Err(changed());
    }
    let now = tx.now_ts();
    let mut deps = Vec::new();
    if let Some(n) = db::source_number(s)? {
        deps.push(n)
    }
    let (effect, mutated) = match &r.operation {
        ManualOperation::Link { target, .. } => {
            let existing = db::outgoing(c, s)?;
            let v = declare(c, a, &owner, target, x, &now)?;
            let key = db::target_key(&v)?;
            let same = existing.iter().find(|e| {
                e.owner == owner
                    && e.origin == "manual"
                    && db::target_key(&e.target).ok().as_ref() == Some(&key)
            });
            if same.is_none() {
                let mut hidden = false;
                for e in existing
                    .iter()
                    .filter(|e| e.owner == owner && e.origin == "manual")
                {
                    match resolve(c, a, &e.target, x) {
                        Ok(_) => (),
                        Err(CollabError::Forbidden(_) | CollabError::NotFound(_)) => hidden = true,
                        Err(e) => return Err(e),
                    }
                }
                if hidden || existing.iter().filter(|e| e.origin == "manual").count() >= 64 {
                    return Err(invalid("restrictedLinkSet"));
                }
            }
            if let Some(n) = db::target_number(&v)? {
                deps.push(n)
            }
            let (id, added) = db::link(c, &owner, s, &v, "manual", &now)?;
            (
                ManualEffect::Linked {
                    edge_id: id.to_string(),
                    added,
                },
                added,
            )
        }
        ManualOperation::Unlink {
            expected_edge_id, ..
        } => {
            let id = tables::revision(expected_edge_id).map_err(|_| invalid("edgeId"))?;
            let e = db::by_id(c, id)?
                .filter(|e| {
                    e.owner == owner
                        && e.origin == "manual"
                        && db::source_key(&e.source).ok() == db::source_key(s).ok()
                })
                .ok_or_else(changed)?;
            resolve(c, a, &e.target, x)?;
            if let Some(n) = db::target_number(&e.target)? {
                deps.push(n)
            }
            db::remove(c, id)?;
            (
                ManualEffect::Unlinked {
                    edge_id: id.to_string(),
                },
                true,
            )
        }
    };
    let after = if mutated {
        db::advance(c, s, &owner)?
    } else {
        before
    };
    let receipt = ManualReceipt {
        scope: r.scope.clone(),
        crew_id: owner.clone(),
        request_id: r.request_id.clone(),
        recorded_at: now,
        intent_digest: digest.clone(),
        source: s.clone(),
        original_manual_link_revision: before.to_string(),
        result_manual_link_revision: after.to_string(),
        event_id: mutated.then(|| uuid::Uuid::new_v4().to_string()),
        effect,
    };
    db::record(
        c,
        db::ReceiptInput {
            crew: &owner,
            principal: &p,
            uuid: &r.request_id,
            method: LINK_SUBMIT,
            digest: &digest,
            input: &r,
            result: &receipt,
            objects: &deps,
        },
    )?;
    if mutated {
        finish(tx, a, &owner)?;
        artifacts::gc(tx.conn())?;
    }
    Ok(receipt)
}
pub fn lookup(
    c: &Connection,
    a: Actor<'_>,
    scope: &Scope,
    crew: &str,
    uuid: &str,
) -> Result<ManualLookup> {
    let p = request(c, a, scope, crew, uuid, false)?;
    let current = objects::scope(c)?;
    let mut retired = false;
    let receipt = match db::receipt(c, crew, &p, uuid)? {
        Some(db::ReadReceipt::Active(s)) if s.method == LINK_SUBMIT => {
            Some(serde_json::from_value::<ManualReceipt>(s.result)?)
        }
        Some(db::ReadReceipt::Active(_)) => return Err(conflict(uuid)),
        Some(db::ReadReceipt::Retired) => {
            retired = true;
            None
        }
        None => None,
    };
    let receipt = receipt.filter(|r| r.scope == *scope);
    Ok(if current != *scope || retired {
        ManualLookup::Historical { receipt }
    } else {
        match receipt {
            Some(receipt) => ManualLookup::Known { receipt },
            None => ManualLookup::NotFound {},
        }
    })
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope: Scope,
    principal: String,
    query: LinkQuery,
    after: String,
    phase: u8,
}
fn hidden_error(e: &CollabError) -> bool {
    matches!(e, CollabError::Forbidden(_) | CollabError::NotFound(_))
}
pub const ADAPTER_SOURCE_BUDGET: u16 = 64;
#[derive(Default, Debug)]
pub(crate) struct WalkStats {
    pub sources_visited: usize,
    pub raw_rows: usize,
    pub workbook_loads: usize,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdapterPosition {
    stage: u8,
    after: String,
}
pub(crate) struct AdapterChunk {
    pub(crate) items: Vec<(AdapterPosition, EdgeView)>,
    pub(crate) next: Option<AdapterPosition>,
    pub(crate) raw_rows: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncomingPosition {
    source: i64,
    slot: AdapterPosition,
}
pub fn list(
    c: &Connection,
    a: Actor<'_>,
    q: ListLinks,
    x: &dyn ExternalTargets,
) -> Result<EdgePage> {
    Ok(list_measured(c, a, q, x)?.0)
}
pub(crate) fn list_measured(
    c: &Connection,
    a: Actor<'_>,
    q: ListLinks,
    x: &dyn ExternalTargets,
) -> Result<(EdgePage, WalkStats)> {
    if !(1..=64).contains(&q.limit) {
        return Err(invalid("limit"));
    }
    let scope = objects::scope(c)?;
    let principal = a.principal(c)?;
    let (phase, after) = if let Some(token) = &q.after {
        let cut: Cursor = serde_json::from_slice(&db::open(c, "managed-link-cursor.v1", token)?)?;
        if cut.scope != scope || cut.principal != principal || cut.query != q.query || cut.phase > 1
        {
            return Err(changed());
        }
        (cut.phase, cut.after)
    } else {
        (0, "0".into())
    };
    let mut p = Projection::new(c, a, x);
    let mut stats = WalkStats::default();
    let mut hidden = false;
    let mut next = None;
    let mut items = Vec::new();
    let (source_ref, revision) = match &q.query {
        LinkQuery::Outgoing {
            source: s,
            include_adapters,
        } => {
            let crew = p.source(s, false)?;
            let mut candidates = Vec::new();
            for e in db::outgoing(c, s)? {
                match p.edge(&e) {
                    Ok(v) => {
                        if phase == 0 && e.id > after.parse::<i64>().map_err(|_| changed())? {
                            candidates.push(v)
                        }
                    }
                    Err(e) if hidden_error(&e) => hidden = true,
                    Err(e) => return Err(e),
                }
            }
            if candidates.len() > q.limit as usize {
                candidates.truncate(q.limit as usize);
                next = Some((0, candidates.last().ok_or_else(changed)?.id.clone()));
            }
            items.extend(candidates);
            if *include_adapters && next.is_none() {
                let pos = if phase == 1 {
                    serde_json::from_str::<AdapterPosition>(&after).map_err(|_| changed())?
                } else {
                    AdapterPosition::default()
                };
                if items.len() == q.limit as usize {
                    next = Some((1, serde_json::to_string(&pos)?));
                } else {
                    let chunk = p.adapter_chunk(s, &pos, ADAPTER_SOURCE_BUDGET as usize)?;
                    stats.raw_rows = chunk.raw_rows;
                    let remaining = q.limit as usize - items.len();
                    if chunk.items.len() > remaining {
                        next = Some((1, serde_json::to_string(&chunk.items[remaining - 1].0)?));
                        items.extend(chunk.items.into_iter().take(remaining).map(|(_, v)| v));
                    } else {
                        items.extend(chunk.items.into_iter().map(|(_, v)| v));
                        next = chunk
                            .next
                            .map(|v| -> Result<_> { Ok((1, serde_json::to_string(&v)?)) })
                            .transpose()?;
                    }
                }
            }
            (Some(s.clone()), Some(db::stamp(c, s, &crew)?.to_string()))
        }
        LinkQuery::Incoming {
            target,
            include_adapters,
        } => {
            p.resolve(target, ResolveMode::Read)?;
            let ask_crew = match a {
                Actor::Human(_) => None,
                Actor::Session(s) if crews::is_management_session(c, s)? => None,
                Actor::Session(s) => Some(sessions::get(c, s)?.crew_id),
            };
            if phase == 0 {
                let n = after.parse::<i64>().map_err(|_| changed())?;
                for e in db::incoming_visible(c, target, n, q.limit + 1, ask_crew.as_deref())? {
                    items.push(p.edge(&e)?)
                }
                if items.len() > q.limit as usize {
                    items.truncate(q.limit as usize);
                    next = Some((0, items.last().ok_or_else(changed)?.id.clone()));
                }
            }
            if *include_adapters && next.is_none() {
                let start = if phase == 1 {
                    serde_json::from_str::<IncomingPosition>(&after).map_err(|_| changed())?
                } else {
                    IncomingPosition {
                        source: 0,
                        slot: AdapterPosition {
                            stage: 2,
                            after: String::new(),
                        },
                    }
                };
                let (added, cut, visited, raw) = walk_incoming_adapters(
                    &mut p,
                    target,
                    start,
                    q.limit as usize - items.len(),
                    ask_crew.as_deref(),
                )?;
                items.extend(added);
                next = cut
                    .map(|v| -> Result<_> { Ok((1, serde_json::to_string(&v)?)) })
                    .transpose()?;
                stats.sources_visited = visited;
                stats.raw_rows = raw;
            }
            (None, None)
        }
    };
    stats.workbook_loads = p.book_loads;
    let next_after = next
        .map(|(phase, after)| -> Result<String> {
            Ok(db::seal(
                c,
                "managed-link-cursor.v1",
                &serde_json::to_vec(&Cursor {
                    scope: scope.clone(),
                    principal: principal.clone(),
                    query: q.query.clone(),
                    phase,
                    after,
                })?,
            )?)
        })
        .transpose()?;
    let outgoing_hidden_set = if hidden {
        Some(OutgoingHiddenSet {
            marker: db::mac(
                c,
                "managed-link-hidden.v1",
                &serde_json::to_vec(&(&scope, &principal, &q.query))?,
            )?,
        })
    } else {
        None
    };
    Ok((
        EdgePage {
            source: source_ref,
            manual_link_revision: revision,
            outgoing_hidden_set,
            items,
            next_after,
        },
        stats,
    ))
}
fn adapter_id(s: &SourceRef, name: &str, t: &TargetRef) -> Result<String> {
    Ok(format!(
        "adapter:{}",
        db::hash(&serde_json::to_vec(&(s, name, t))?)
    ))
}
fn source_ref(n: i64, k: &str) -> Result<SourceRef> {
    Ok(match k {
        "plan" => SourceRef::Work {
            id: format!("W{n}"),
        },
        "requirement" => SourceRef::Requirement {
            id: format!("#{n}"),
        },
        "human_task" => SourceRef::Ask {
            id: format!("H{n}"),
        },
        "decision" => SourceRef::Decision {
            id: format!("D{n}"),
        },
        "risk" => SourceRef::Risk {
            id: format!("R{n}"),
        },
        "lesson" => SourceRef::Lesson {
            id: format!("L{n}"),
        },
        "table" => SourceRef::Table {
            id: format!("T{n}"),
        },
        "custom_row" => SourceRef::Row {
            id: format!("O{n}"),
        },
        _ => return Err(invalid("adapterSource")),
    })
}
fn walk_incoming_adapters(
    p: &mut Projection<'_>,
    target: &TargetRef,
    initial: IncomingPosition,
    remaining: usize,
    ask_crew: Option<&str>,
) -> Result<(Vec<EdgeView>, Option<IncomingPosition>, usize, usize)> {
    if initial.source < 0 || initial.slot.stage > 2 {
        return Err(changed());
    }
    let records = db::adapter_sources(
        p.c,
        initial.source,
        initial.source > 0 && initial.slot.stage < 2,
        ADAPTER_SOURCE_BUDGET,
        ask_crew,
    )?;
    let at_budget = records.len() == ADAPTER_SOURCE_BUDGET as usize;
    let mut out = Vec::new();
    let mut last = initial.clone();
    let mut visited = 0;
    let mut raw = 0;
    let target_key = db::target_key(target)?;
    for (number, kind) in records {
        if raw == usize::from(ADAPTER_SOURCE_BUDGET) {
            return Ok((out, Some(last), visited, raw));
        }
        visited += 1;
        let source = source_ref(number, &kind)?;
        let slot = if number == initial.source && initial.slot.stage < 2 {
            initial.slot.clone()
        } else {
            AdapterPosition::default()
        };
        let chunk = p.adapter_chunk(&source, &slot, usize::from(ADAPTER_SOURCE_BUDGET) - raw)?;
        raw += chunk.raw_rows;
        for (position, v) in chunk.items {
            if db::target_key(&v.target)? != target_key {
                continue;
            }
            if out.len() == remaining {
                return Ok((out, Some(last), visited, raw));
            }
            last = IncomingPosition {
                source: number,
                slot: position,
            };
            out.push(v);
        }
        if let Some(slot) = chunk.next {
            return Ok((
                out,
                Some(IncomingPosition {
                    source: number,
                    slot,
                }),
                visited,
                raw,
            ));
        }
        last = IncomingPosition {
            source: number,
            slot: AdapterPosition {
                stage: 2,
                after: String::new(),
            },
        };
    }
    Ok((out, if at_budget { Some(last) } else { None }, visited, raw))
}

/// Trusted lifecycle fact inspector: no caller authorization is replayed here.
pub trait KnowledgeObjectState {
    fn exists(
        &self,
        reference: &crewd_proto::team_data::DataRef,
        require_document: bool,
    ) -> Result<bool>;
}
pub fn reconcile_knowledge_namespace(
    tx: &mut Tx<'_>,
    current: &crewd_proto::team_data::DataContext,
    inspector: &dyn KnowledgeObjectState,
) -> Result<()> {
    let c = tx.conn();
    let edges = db::knowledge_edges(c, &current.crew_id, &current.namespace)?;
    let mut rows = HashSet::new();
    let mut sources = HashSet::new();
    let mut crews = HashSet::new();
    for e in edges {
        let (old, reference, document) = match &e.target {
            TargetRef::Document { source } => (&source.context, &source.reference, true),
            TargetRef::Knowledge { source } => (&source.context, &source.reference, false),
            _ => return Err(invalid("knowledgeEdge")),
        };
        let same = old.db_id == current.db_id
            && old.authority_id == current.authority_id
            && old.restore_epoch == current.restore_epoch
            && old.crew_id == current.crew_id
            && old.namespace == current.namespace;
        if same && inspector.exists(reference, document)? {
            continue;
        }
        db::retire_knowledge(c, &e.target)?;
        db::remove(c, e.id)?;
        crews.insert(e.owner.clone());
        if e.origin.starts_with("column:") {
            if let Some(n) = db::source_number(&e.source)? {
                rows.insert(n);
            }
        } else {
            sources.insert((db::source_key(&e.source)?, e.owner));
        }
    }
    for n in rows {
        db::enqueue_row(c, n)?;
    }
    for (source, crew) in sources {
        let source: SourceRef = serde_json::from_str(&source)?;
        db::enqueue_source(c, &source, &crew)?;
    }
    let _ = crews;
    Ok(())
}
/// Call before the actual crew cascade, then emit one crew-only hint per result
/// after success in the same Tx. No hint or user progress on failed purge.
pub fn affected_by_crew_purge(c: &Connection, crew: &str) -> Result<Vec<String>> {
    Ok(db::affected_by_crew_purge(c, crew)?)
}

/// Entry check must not silently repair an unknown committed queue.
pub fn check_target_deletions(c: &Connection) -> Result<()> {
    if tables::enabled(c)? && !db::deletions_empty(c)? {
        return Err(StoreError::Invalid("unsettled managed link deletion witness".into()).into());
    }
    Ok(())
}
/// Required on successful collab::run exit, before commit. No progress/chat.
pub fn settle_target_deletions(tx: &mut Tx<'_>) -> Result<()> {
    if !tables::enabled(tx.conn())? {
        return Ok(());
    }
    for crew_id in db::settle_deletions(tx.conn())? {
        tx.note(Change::ManagedLinksChanged { crew_id });
    }
    Ok(())
}
