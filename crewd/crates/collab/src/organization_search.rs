//! Retained ordering cuts, current facts and current permissions. Reads have no effects.
use crate::{
    CollabError, Result, Tx,
    managed_links::{DenyExternal, Projection, ResolveMode},
    managed_objects::{self, Actor},
    refusal::{Code, Refusal},
};
use crewd_proto::{
    managed_objects::{Object, Scope},
    managed_tables_v2::*,
    organization_search::*,
};
use crewd_store::organization_search as db;
use rusqlite::{Connection, OptionalExtension, params, types::Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const FACT_BUDGET: u16 = 128;
#[derive(Clone, Debug)]
pub struct ReadClock {
    pub wall_ms: i64,
    pub elapsed_ms: u64,
    pub boot_id: String,
}

fn invalid() -> CollabError {
    CollabError::Invalid(Refusal::new(Code::SearchInvalid).arg("field", "conditions"))
}
fn expired() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::SearchCursorExpired))
}
fn unavailable() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::SearchUnavailable))
}
fn hidden(e: &CollabError) -> bool {
    matches!(e, CollabError::NotFound(_) | CollabError::Forbidden(_))
}
fn clip(s: &str) -> String {
    s.chars().take(MAX_EXCERPT_CHARACTERS).collect()
}
fn norm_time(t: Option<&str>) -> Result<Option<String>> {
    t.map(|v| {
        chrono::DateTime::parse_from_rfc3339(v)
            .map(|v| crewd_store::ts_from_ms(v.timestamp_millis()))
            .map_err(|_| invalid())
    })
    .transpose()
}
fn slot(field: &str, on: MatchOn, s: impl Into<String>) -> db::Slot {
    db::Slot {
        field: field.into(),
        matched_on: on,
        text: s.into(),
    }
}
fn id(t: &TargetRef) -> Result<String> {
    Ok(match t {
        TargetRef::Work { id }
        | TargetRef::Requirement { id }
        | TargetRef::Ask { id }
        | TargetRef::Table { id }
        | TargetRef::Row { id }
        | TargetRef::Decision { id }
        | TargetRef::Risk { id }
        | TargetRef::Lesson { id }
        | TargetRef::Crew { id }
        | TargetRef::Session { id }
        | TargetRef::Channel { id }
        | TargetRef::Message { id } => id.clone(),
        TargetRef::Artifact { reference } => reference.key.clone(),
        TargetRef::Human {} => "human".into(),
        _ => return Err(invalid()),
    })
}
fn kind(t: &TargetRef) -> Option<Kind> {
    Some(match t {
        TargetRef::Work { .. } => Kind::Work,
        TargetRef::Requirement { .. } => Kind::Requirement,
        TargetRef::Ask { .. } => Kind::Ask,
        TargetRef::Table { .. } => Kind::Table,
        TargetRef::Row { .. } => Kind::Row,
        TargetRef::Decision { .. } => Kind::Decision,
        TargetRef::Risk { .. } => Kind::Risk,
        TargetRef::Lesson { .. } => Kind::Lesson,
        TargetRef::Artifact { .. } => Kind::Artifact,
        TargetRef::Message { .. } => Kind::Message,
        _ => return None,
    })
}
fn source_ref(t: &TargetRef, crew: Option<&str>) -> Option<SourceRef> {
    Some(match t {
        TargetRef::Work { id } => SourceRef::Work { id: id.clone() },
        TargetRef::Requirement { id } => SourceRef::Requirement { id: id.clone() },
        TargetRef::Ask { id } => SourceRef::Ask { id: id.clone() },
        TargetRef::Table { id } => SourceRef::Table { id: id.clone() },
        TargetRef::Row { id } => SourceRef::Row { id: id.clone() },
        TargetRef::Decision { id } => SourceRef::Decision { id: id.clone() },
        TargetRef::Risk { id } => SourceRef::Risk { id: id.clone() },
        TargetRef::Lesson { id } => SourceRef::Lesson { id: id.clone() },
        TargetRef::Artifact { reference } => SourceRef::Artifact {
            reference: reference.clone(),
            metadata_crew_id: crew?.into(),
        },
        _ => return None,
    })
}
fn source_target(s: &SourceRef) -> TargetRef {
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

struct Views<'a> {
    p: Projection<'a>,
    tables: HashMap<String, Table>,
    indexing: bool,
}
impl<'a> Views<'a> {
    fn new(c: &'a Connection, a: Actor<'a>) -> Self {
        Self {
            p: Projection::new(c, a, &DenyExternal),
            tables: HashMap::new(),
            indexing: false,
        }
    }
    fn for_index(c: &'a Connection) -> Self {
        let mut v = Self::new(c, Actor::Human("organization-search-index"));
        v.indexing = true;
        v
    }
    fn table(&mut self, id: &str) -> Result<Table> {
        if let Some(t) = self.tables.get(id) {
            return Ok(t.clone());
        }
        let t = crewd_store::managed_tables_v2::table(
            self.p.c,
            crewd_store::managed_tables_v2::number(id, 'T')?,
        )?
        .ok_or_else(unavailable)?;
        self.tables.insert(id.into(), t.clone());
        Ok(t)
    }
    fn project(&mut self, t: &TargetRef) -> Result<db::Document> {
        let resolved = self.p.resolve(t, ResolveMode::Read)?;
        let c = self.p.c;
        let mut title_field = String::new();
        let mut d = db::Document {
            revision: None,
            target: t.clone(),
            kind: kind(t),
            owner_crew: resolved.owner_crew_id,
            title: resolved.summary,
            table_id: None,
            channel_id: None,
            changed_at: None,
            status: Status::Current,
            system_default: false,
            slots: Vec::new(),
        };
        match t {
            TargetRef::Work { id } => {
                let (_, w) = self.p.work(id)?;
                d.revision = Some(w.revision.to_string());
                d.title = w.title;
                d.changed_at = Some(crewd_store::ts_from_ms(w.updated_at.0));
                if matches!(
                    w.status,
                    crewd_work::Status::Done | crewd_work::Status::Dropped
                ) {
                    d.status = Status::Historical
                }
                for (f, text) in [("goal", w.body.goal), ("acceptance", w.body.acceptance)] {
                    d.slots.push(slot(f, MatchOn::Text, text))
                }
                if let Some(n) = w.last_report {
                    d.slots.push(slot("report", MatchOn::Text, n.text))
                }
            }
            TargetRef::Requirement { id } => {
                let r = crate::requirements::reference(c, id)?;
                let b = self.p.book()?;
                let detail = crate::requirements::detail(c, &r, b)?;
                d.revision = Some(detail.todo_attributes.revision.clone());
                d.title = detail.todo.text;
                d.changed_at = detail
                    .closure
                    .as_ref()
                    .map(|v| v.at.clone())
                    .or(Some(detail.todo.created_at));
                if detail.todo.status == "done" || detail.closure.is_some() {
                    d.status = Status::Historical
                }
            }
            TargetRef::Ask { id } => {
                let (_, h) = self.p.ask(id)?;
                d.revision = Some(crewd_store::managed_links::hash(&serde_json::to_vec(
                    &h.state,
                )?));
                d.title = h.question;
                let at = match h.state {
                    crewd_work::AskState::Open => h.asked_at,
                    crewd_work::AskState::Answered { at, .. }
                    | crewd_work::AskState::Defaulted { at }
                    | crewd_work::AskState::Withdrawn { at, .. } => {
                        d.status = Status::Historical;
                        at
                    }
                };
                d.changed_at = Some(crewd_store::ts_from_ms(at.0));
            }
            TargetRef::Decision { id } | TargetRef::Risk { id } | TargetRef::Lesson { id } => {
                match managed_objects::get_with_defaults(c, self.p.actor, id, true)? {
                    Object::Decision { decision: o } => {
                        d.revision = Some(o.revision.clone());
                        d.title = o.draft.question;
                        d.changed_at = Some(if let Some(next_id) = &o.superseded_by {
                            // A pre-index replacement is still an exact retained
                            // mutation witness: its successor was born in the
                            // same transaction that superseded this decision.
                            let Object::Decision { decision: next } =
                                managed_objects::get_with_defaults(c, self.p.actor, next_id, true)?
                            else {
                                return Err(unavailable());
                            };
                            if next.supersedes.as_deref() != Some(o.id.as_str())
                                || next.crew_id != o.crew_id
                            {
                                return Err(unavailable());
                            }
                            next.created_at
                        } else {
                            o.withdrawal
                                .as_ref()
                                .map(|w| w.at.clone())
                                .unwrap_or(o.created_at)
                        });
                        d.status =
                            if o.status == crewd_proto::managed_objects::DecisionStatus::Active {
                                Status::Current
                            } else {
                                Status::Historical
                            };
                        d.system_default = o.author.kind == "systemDefault";
                        for (f, text) in
                            [("choice", o.draft.choice), ("rationale", o.draft.rationale)]
                        {
                            d.slots.push(slot(f, MatchOn::Text, text))
                        }
                        if let Some(v) = o.draft.alternatives {
                            d.slots.push(slot("alternatives", MatchOn::Text, v))
                        }
                    }
                    Object::Risk { risk: o } => {
                        d.revision = Some(o.revision.clone());
                        d.title = o.draft.summary;
                        d.changed_at = Some(o.updated_at);
                        if matches!(
                            o.status,
                            crewd_proto::managed_objects::RiskStatus::Closed
                                | crewd_proto::managed_objects::RiskStatus::Occurred
                        ) {
                            d.status = Status::Historical
                        }
                        if let Some(v) = o.draft.mitigation {
                            d.slots.push(slot("mitigation", MatchOn::Text, v))
                        }
                        if let Some(v) = o.status_reason {
                            d.slots.push(slot("reason", MatchOn::Text, v))
                        }
                    }
                    Object::Lesson { lesson: o } => {
                        d.revision = Some(o.revision.clone());
                        d.title = o.draft.lesson;
                        d.changed_at = if o.revision == "1" {
                            Some(o.created_at.clone())
                        } else {
                            receipt_time(c, id, &o.revision)?
                        };
                        if o.status == crewd_proto::managed_objects::LessonStatus::Stale {
                            d.status = Status::Historical
                        }
                        d.slots
                            .push(slot("happened", MatchOn::Text, o.draft.happened));
                        if let Some(v) = o.draft.detection {
                            d.slots.push(slot("detection", MatchOn::Text, v))
                        }
                        for (i, v) in o.draft.evidence.into_iter().enumerate() {
                            d.slots
                                .push(slot(&format!("evidence:{i}"), MatchOn::Text, v))
                        }
                    }
                    _ => return Err(unavailable()),
                }
            }
            TargetRef::Table { id } => {
                let table = self.table(id)?;
                d.revision = Some(table.schema_revision.clone());
                d.title = table.title;
                if let Some(v) = table.purpose {
                    d.slots.push(slot("purpose", MatchOn::Text, v))
                }
                for f in table.fields.into_iter().filter(|f| !f.hidden) {
                    d.slots.push(slot(&f.id, MatchOn::Text, f.name))
                }
                d.changed_at = source_time(
                    c,
                    id,
                    Some(table.schema_revision.parse().map_err(|_| unavailable())?),
                )?;
            }
            TargetRef::Row { id } => {
                let (t, revision, _, cells) = crewd_store::managed_tables_v2::row_base(
                    c,
                    crewd_store::managed_tables_v2::number(id, 'O')?,
                )?
                .ok_or_else(unavailable)?;
                self.tables.insert(t.id.clone(), t.clone());
                title_field = t.title_field_id.clone();
                d.revision = Some(revision.clone());
                d.table_id = Some(t.id.clone());
                d.changed_at =
                    source_time(c, id, Some(revision.parse().map_err(|_| unavailable())?))?;
                for cell in cells {
                    if t.fields
                        .iter()
                        .any(|f| f.id == cell.field_id && (self.indexing || !f.hidden))
                    {
                        let value = match cell.value {
                            Cell::Text(s) | Cell::Integer(s) => s,
                            Cell::Real(n) => n.to_string(),
                            Cell::Null => continue,
                        };
                        if cell.field_id == t.title_field_id {
                            d.title = value.clone()
                        }
                        d.slots.push(slot(&cell.field_id, MatchOn::Text, value))
                    }
                }
            }
            TargetRef::Message { id } => {
                let m = crate::messages::get(c, id)?;
                if let Actor::Session(session) = self.p.actor
                    && !crate::channels::readable_by(c, &m.channel_id, session)?
                {
                    return Err(CollabError::Forbidden(Refusal::new(
                        Code::ResourceForbidden,
                    )));
                }
                d.title = clip(&m.text);
                d.slots.push(slot("body", MatchOn::Text, m.text));
                d.channel_id = Some(m.channel_id);
                d.changed_at = Some(m.created_at);
            }
            TargetRef::Artifact { reference } => {
                let a =
                    crewd_store::managed_artifacts::get(c, reference)?.ok_or_else(unavailable)?;
                d.title = reference.key.clone();
                d.changed_at = Some(a.created_at);
            }
            TargetRef::Crew { id } => {
                let o = crate::crews::get(c, id)?;
                d.title = o.title;
                d.changed_at = Some(o.updated_at);
            }
            TargetRef::Session { id } => {
                let o = crate::sessions::get(c, id)?;
                d.changed_at = Some(o.updated_at);
                d.title = o.title.or(o.display_name).unwrap_or_else(|| id.clone());
            }
            TargetRef::Channel { id } => {
                let o = crate::channels::get(c, id)?;
                d.title = o.name;
            }
            TargetRef::Human {} => {
                d.title = "human".into();
            }
            TargetRef::Document { .. } | TargetRef::Knowledge { .. } => return Err(invalid()),
        }
        if let Some(source) = source_key_for_target(t)?
            && let Some((at, revision)) = db::mutation_time(c, &source)?
        {
            d.changed_at = at;
            d.revision = Some(format!(
                "{}:{revision}",
                d.revision.as_deref().unwrap_or("source")
            ));
        }
        if let Some(at) = &d.changed_at
            && !chrono::DateTime::parse_from_rfc3339(at)
                .is_ok_and(|v| crewd_store::ts_from_ms(v.timestamp_millis()) == *at)
        {
            d.changed_at = None;
        }
        if let Some(crew) = &d.owner_crew
            && crate::crews::is_archived(c, crew)?
        {
            d.status = Status::Archived
        }
        d.slots
            .insert(0, slot(&title_field, MatchOn::Title, d.title.clone()));
        d.slots.push(slot("reference", MatchOn::Reference, id(t)?));
        Ok(d)
    }
}
fn source_time(c: &Connection, id: &str, revision: Option<i64>) -> Result<Option<String>> {
    let n = id
        .get(1..)
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(invalid)?;
    if revision.is_some_and(|r| r > 1) {
        return receipt_time(c, id, &revision.unwrap().to_string());
    }
    Ok(c.query_row(
        "SELECT created_at FROM object_refs WHERE number=?1",
        [n],
        |r| r.get(0),
    )
    .optional()?)
}
fn receipt_time(c: &Connection, id: &str, revision: &str) -> Result<Option<String>> {
    if id.starts_with('T') {
        let n = crewd_store::managed_tables_v2::number(id, 'T')?;
        return Ok(c.query_row("SELECT e.created_at FROM managed_table_schema_events s JOIN managed_link_events e ON e.id=s.event_id WHERE s.table_number=?1 AND json_extract(s.after_json,'$.schemaRevision')=?2 LIMIT 1",params![n,revision],|r|r.get(0)).optional()?);
    }
    let v2:Option<Option<String>>=c.query_row("SELECT json_extract(result_json,'$.recordedAt') FROM managed_v2_receipts WHERE retired=0 AND json_extract(result_json,'$.effect.rowId')=?1 AND json_extract(result_json,'$.effect.resultRowRevision')=?2 LIMIT 1",params![id,revision],|r|r.get(0)).optional()?;
    if let Some(Some(at)) = v2 {
        return Ok(Some(at));
    }
    let arm = match id.chars().next() {
        Some('O') => "row",
        Some('D') => "decision",
        Some('R') => "risk",
        Some('L') => "lesson",
        _ => return Ok(None),
    };
    let sql = format!(
        "SELECT json_extract(result_json,'$.recordedAt') FROM managed_object_receipts WHERE json_extract(result_json,'$.object.{arm}.id')=?1 AND json_extract(result_json,'$.object.{arm}.revision')=?2 LIMIT 1"
    );
    Ok(c.query_row(&sql, params![id, revision], |r| {
        r.get::<_, Option<String>>(0)
    })
    .optional()?
    .flatten())
}
fn source_key_for_target(t: &TargetRef) -> Result<Option<String>> {
    Ok(Some(match t {
        TargetRef::Work { id }
        | TargetRef::Requirement { id }
        | TargetRef::Ask { id }
        | TargetRef::Table { id }
        | TargetRef::Row { id }
        | TargetRef::Decision { id }
        | TargetRef::Risk { id }
        | TargetRef::Lesson { id } => db::source_object(
            id.get(1..)
                .and_then(|s| s.parse().ok())
                .ok_or_else(invalid)?,
        ),
        TargetRef::Message { id } => db::source_message(id),
        TargetRef::Artifact { reference } => {
            db::source_artifact(&serde_json::to_string(reference)?)
        }
        TargetRef::Crew { id } => format!("c:{id}"),
        TargetRef::Session { id } => format!("s:{id}"),
        TargetRef::Channel { id } => format!("h:{id}"),
        TargetRef::Human {} => "human".into(),
        _ => return Ok(None),
    }))
}

fn target_for_source(c: &Connection, source: &str) -> Result<Option<TargetRef>> {
    if let Some(v) = source.strip_prefix("o:") {
        let n = v.parse::<i64>().map_err(|_| unavailable())?;
        let k: Option<String> = c
            .query_row("SELECT kind FROM object_refs WHERE number=?1", [n], |r| {
                r.get(0)
            })
            .optional()?;
        return k
            .map(|k| {
                Ok(match k.as_str() {
                    "plan" => TargetRef::Work {
                        id: format!("W{n}"),
                    },
                    "requirement" => TargetRef::Requirement {
                        id: format!("#{n}"),
                    },
                    "human_task" => TargetRef::Ask {
                        id: format!("H{n}"),
                    },
                    "decision" => TargetRef::Decision {
                        id: format!("D{n}"),
                    },
                    "risk" => TargetRef::Risk {
                        id: format!("R{n}"),
                    },
                    "lesson" => TargetRef::Lesson {
                        id: format!("L{n}"),
                    },
                    "table" => TargetRef::Table {
                        id: format!("T{n}"),
                    },
                    "custom_row" => TargetRef::Row {
                        id: format!("O{n}"),
                    },
                    _ => return Err(unavailable()),
                })
            })
            .transpose();
    }
    if let Some(id) = source.strip_prefix("m:") {
        return Ok(crewd_store::messages::get(c, id)?.map(|_| TargetRef::Message { id: id.into() }));
    }
    if let Some(r) = source.strip_prefix("a:") {
        let reference = serde_json::from_str(r)?;
        return Ok(crewd_store::managed_artifacts::get(c, &reference)?
            .map(|_| TargetRef::Artifact { reference }));
    }
    for (prefix, table) in [("c:", "crews"), ("s:", "sessions"), ("h:", "channels")] {
        if let Some(id) = source.strip_prefix(prefix) {
            let yes: bool = c.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
                [id],
                |r| r.get(0),
            )?;
            return Ok(yes.then(|| match prefix {
                "c:" => TargetRef::Crew { id: id.into() },
                "s:" => TargetRef::Session { id: id.into() },
                _ => TargetRef::Channel { id: id.into() },
            }));
        }
    }
    if source == "human" {
        return Ok(Some(TargetRef::Human {}));
    }
    Err(unavailable())
}
fn binding<'a>(source: &'a str) -> db::Binding<'a> {
    let mut b = db::Binding::default();
    if let Some(n) = source.strip_prefix("o:") {
        b.object = n.parse().ok()
    } else if let Some(s) = source.strip_prefix("m:") {
        b.message = Some(s)
    } else if let Some(s) = source.strip_prefix("a:") {
        b.artifact = Some(s)
    } else if let Some(s) = source.strip_prefix("c:") {
        b.crew = Some(s)
    } else if let Some(s) = source.strip_prefix("s:") {
        b.session = Some(s)
    } else if let Some(s) = source.strip_prefix("h:") {
        b.channel = Some(s)
    }
    b
}

/// Small real mutations settle in their own committing writer transaction.
pub fn flush(tx: &mut Tx<'_>) -> Result<()> {
    if !db::enabled(tx.conn())? || tx.organization_search_budget == 0 {
        return Ok(());
    }
    let c = tx.conn();
    let span = if tx.organization_search_maintenance {
        None
    } else {
        Some(db::span(c)?.ok_or_else(unavailable)?)
    };
    let budget = tx.organization_search_budget.min(64);
    let mut remaining = budget;
    let mut views = Views::for_index(c);
    for (source, revision) in db::dirty_for(c, span.as_deref(), budget as u16)? {
        if let Some(target) = target_for_source(c, &source)? {
            let doc = views.project(&target)?;
            db::put(c, &source, binding(&source), &doc)?;
            db::graph_job(c, &source, &db::fingerprint(&doc)?)?;
        } else {
            db::remove(c, &source)?;
        }
        db::clear_dirty(c, &source, revision)?;
        remaining -= 1;
    }
    if remaining > 0 {
        remaining -= advance_graph(c, &mut views, span.as_deref(), remaining as u16)?;
    }
    if remaining > 0 {
        remaining -= db::gc_step_for(c, span.as_deref(), remaining as u16)?;
    }
    if remaining != budget {
        db::finish_publication(c)?;
    }
    tx.organization_search_budget = remaining;
    Ok(())
}
pub fn maintenance_needed(c: &Connection, _now_ms: i64) -> Result<bool> {
    if !db::enabled(c)? {
        return Ok(false);
    }
    Ok(db::state(c)?.building)
}
pub fn maintenance_step(tx: &mut Tx<'_>, limit: usize) -> Result<bool> {
    if !(1..=64).contains(&limit) {
        return Err(invalid());
    }
    if !db::enabled(tx.conn())? {
        return Ok(false);
    }
    tx.organization_search_maintenance = true;
    tx.organization_search_budget = limit;
    if db::gc_pending(tx.conn())? {
        let used = db::gc_step(tx.conn(), limit.div_ceil(2) as u16)?;
        tx.organization_search_budget -= used;
    }
    let available = tx.organization_search_budget;
    if available > 0
        && db::dirty(tx.conn(), 1)?.is_empty()
        && db::graph_jobs(tx.conn(), 1)?.is_empty()
    {
        let n = if db::state(tx.conn())?.phase < 8 {
            db::seed_step(tx.conn(), available as u16)?
        } else {
            db::fanout_step(tx.conn(), available as u16)?
        };
        tx.organization_search_budget = available.saturating_sub(n);
    }
    flush(tx)?;
    db::prune(tx.conn())?;
    db::finish_publication(tx.conn())?;
    Ok(db::state(tx.conn())?.building)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope: Scope,
    principal: String,
    generation: String,
    query: String,
    cut: i64,
    issued: i64,
    expires: i64,
    elapsed_issued: u64,
    elapsed_deadline: u64,
    boot_id: String,
    position: Option<db::Position>,
    edge: Option<db::GraphPosition>,
}

fn cursor(
    c: &Connection,
    a: Actor<'_>,
    query: &str,
    token: Option<&str>,
    scope: &Scope,
    clock: &ReadClock,
) -> Result<Cursor> {
    if clock.boot_id.is_empty()
        || clock.boot_id.len() > 256
        || clock.boot_id.chars().any(char::is_control)
    {
        return Err(unavailable());
    }
    let now = clock.wall_ms;
    let state = db::state(c).map_err(|_| unavailable())?;
    let principal = a.principal(c)?;
    if let Some(token) = token {
        let body = crewd_store::managed_links::open(c, "organization-search-cursor.v1", token)
            .map_err(|_| expired())?;
        let v: Cursor = serde_json::from_slice(&body).map_err(|_| expired())?;
        if v.scope != *scope
            || v.principal != principal
            || v.query != query
            || v.generation != state.generation
            || v.cut < state.min_cut
            || v.cut > state.clock
            || clock.boot_id != v.boot_id
            || clock.elapsed_ms < v.elapsed_issued
            || clock.elapsed_ms >= v.elapsed_deadline
        {
            return Err(expired());
        }
        Ok(v)
    } else {
        Ok(Cursor {
            scope: scope.clone(),
            principal,
            generation: state.generation,
            query: query.into(),
            cut: state.clock,
            issued: now,
            elapsed_issued: clock.elapsed_ms,
            elapsed_deadline: clock
                .elapsed_ms
                .checked_add(CURSOR_LIFETIME_MS as u64)
                .ok_or_else(unavailable)?,
            boot_id: clock.boot_id.clone(),
            expires: now.checked_add(CURSOR_LIFETIME_MS).ok_or_else(invalid)?,
            position: None,
            edge: None,
        })
    }
}
fn encode(c: &Connection, v: &Cursor) -> Result<String> {
    Ok(crewd_store::managed_links::seal(
        c,
        "organization-search-cursor.v1",
        &serde_json::to_vec(v)?,
    )?)
}
fn snapshot(v: &Cursor) -> Snapshot {
    Snapshot {
        sorted_at: crewd_store::ts_from_ms(v.issued),
        expires_at: crewd_store::ts_from_ms(v.expires),
    }
}
fn fence(c: &Connection, a: Actor<'_>, scope: Option<&Scope>) -> Result<Scope> {
    a.principal(c)?;
    if !db::enabled(c)? {
        return Err(unavailable());
    }
    let current = managed_objects::scope(c)?;
    if scope.is_some_and(|s| *s != current) {
        return Err(expired());
    }
    Ok(current)
}
fn validate(c: &Connection, q: &mut Search) -> Result<Vec<Kind>> {
    q.query = q.query.trim().into();
    if q.query.is_empty()
        || q.query.len() > MAX_QUERY_BYTES
        || q.query.chars().count() > MAX_QUERY_CHARACTERS
        || q.query.contains('\0')
        || !(1..=MAX_LIMIT).contains(&q.limit)
    {
        return Err(invalid());
    }
    let all = [
        Kind::Decision,
        Kind::Risk,
        Kind::Lesson,
        Kind::Work,
        Kind::Requirement,
        Kind::Ask,
        Kind::Table,
        Kind::Row,
        Kind::Artifact,
        Kind::Message,
    ];
    let mut kinds = q.kinds.clone().unwrap_or(all.to_vec());
    if kinds.is_empty() {
        return Err(invalid());
    }
    kinds.retain(|k| match q.r#in {
        Domain::Messages => *k == Kind::Message,
        Domain::Objects => *k != Kind::Message,
        Domain::All => true,
    });
    if q.table_id.is_some() && q.channel_id.is_some() {
        return Err(invalid());
    }
    if let Some(t) = &q.table_id {
        crewd_store::managed_tables_v2::table(
            c,
            crewd_store::managed_tables_v2::number(t, 'T').map_err(|_| invalid())?,
        )?
        .ok_or_else(invalid)?;
        kinds.retain(|k| *k == Kind::Row)
    }
    if let Some(ch) = &q.channel_id {
        crate::channels::get(c, ch)?;
        kinds.retain(|k| *k == Kind::Message)
    }
    if kinds.is_empty() {
        return Err(invalid());
    }
    if let Some(crew) = &q.crew_id {
        crate::crews::get(c, crew)?;
    }
    q.after_time = norm_time(q.after_time.as_deref())?;
    q.before_time = norm_time(q.before_time.as_deref())?;
    if matches!((&q.after_time,&q.before_time),(Some(a),Some(b)) if a>=b) {
        return Err(invalid());
    }
    Ok(kinds)
}
fn qualification(
    c: &Connection,
    a: Actor<'_>,
    q: &Search,
    kinds: &[Kind],
    now: i64,
) -> Result<(String, Vec<Value>)> {
    let mut sql = Vec::new();
    let mut values = Vec::new();
    let mut bind = |v: Value| {
        values.push(v);
        format!("?{}", values.len() + 6)
    };
    let selected = kinds
        .iter()
        .map(|k| Ok(bind(serde_json::to_string(k)?.into())))
        .collect::<Result<Vec<_>>>()?;
    sql.push(format!("v.kind IN({})", selected.join(",")));
    if !q.include_system_defaults {
        sql.push("v.system_default=0".into())
    }
    if let Some(t) = &q.table_id {
        sql.push(format!("EXISTS(SELECT 1 FROM organization_search_versions current WHERE current.source=v.source AND current.valid_to IS NULL AND current.table_id={})", bind(t.clone().into())))
    }
    if let Some(v) = &q.crew_id {
        sql.push(format!("EXISTS(SELECT 1 FROM organization_search_versions current WHERE current.source=v.source AND current.valid_to IS NULL AND current.owner_crew={})", bind(v.clone().into())))
    }
    if let Some(v) = &q.channel_id {
        sql.push(format!("EXISTS(SELECT 1 FROM organization_search_versions current WHERE current.source=v.source AND current.valid_to IS NULL AND current.channel_id={})", bind(v.clone().into())))
    }
    if let Some(v) = &q.after_time {
        sql.push(format!("v.changed_at>={}", bind(v.clone().into())))
    }
    if let Some(v) = &q.before_time {
        sql.push(format!("v.changed_at<{}", bind(v.clone().into())))
    }
    let text_arg = if q.query.chars().count() >= 3 {
        format!("\"{}\"", q.query.replace('"', "\"\""))
    } else {
        q.query.clone()
    };
    let p = bind(text_arg.into());
    let active = "(h.kind!='\"row\"' OR s.slot_kind='\"reference\"' OR EXISTS(SELECT 1 FROM managed_tables t,json_each(t.fields_json) f WHERE t.number=CAST(substr(h.table_id,2) AS INTEGER) AND json_extract(f.value,'$.id')=s.field_id AND json_extract(f.value,'$.hidden')=0))";
    if q.query.chars().count() >= 3 {
        sql.push(format!("v.source IN(SELECT h.source FROM organization_search_fts f JOIN organization_search_slots s ON s.id=f.rowid JOIN organization_search_versions h ON h.id=s.version_id WHERE organization_search_fts MATCH {p} AND h.id=v.id AND {active} UNION SELECT 'o:'||printf('%020d',a.table_number) FROM organization_search_alias_fts f JOIN organization_search_aliases a ON a.id=f.rowid WHERE organization_search_alias_fts MATCH {p} AND a.visible_from<=?1)"));
    } else {
        sql.push(format!("v.source IN(SELECT h.source FROM organization_search_slots s JOIN organization_search_versions h ON h.id=s.version_id WHERE instr(lower(s.body),lower({p}))>0 AND h.id=v.id AND {active} UNION SELECT 'o:'||printf('%020d',a.table_number) FROM organization_search_aliases a WHERE instr(lower(a.label),lower({p}))>0 AND a.visible_from<=?1)"));
    }
    if let Actor::Session(session) = a {
        let plan = crate::channels::ReadPlan::of(c, session, now)?;
        let (expr, args) = plan.sql("ch", values.len() + 7);
        values.extend(args);
        sql.push(format!("(v.kind!='\\\"message\\\"' OR EXISTS(SELECT 1 FROM messages m JOIN channels ch ON ch.id=m.channel_id WHERE m.id=src.message_id AND {expr}))").replace("\\\"","\""));
        let plan = crate::resource_acl::ReadPlan::of(c, &crate::Sender::Session(session.into()))?;
        let (predicate, args) = plan.sql("r.crew_id", values.len() + 7);
        values.extend(args);
        sql.push(format!("(v.kind!='\"ask\"' OR EXISTS(SELECT 1 FROM object_refs r WHERE r.number=src.object_number AND {predicate}))"));
    }
    Ok((sql.join(" AND "), values))
}
fn endpoint_qualification(
    c: &Connection,
    a: Actor<'_>,
    alias: &str,
    start: usize,
    now: i64,
) -> Result<(String, Vec<Value>)> {
    let Actor::Session(session) = a else {
        return Ok(("1".into(), vec![]));
    };
    let plan = crate::channels::ReadPlan::of(c, session, now)?;
    let (message, args) = plan.sql("ch", start);
    let mut values = args;
    let plan = crate::resource_acl::ReadPlan::of(c, &crate::Sender::Session(session.into()))?;
    let (ask, args) = plan.sql("r.crew_id", start + values.len());
    values.extend(args);
    let sql = format!(
        "((json_extract({alias}.target,'$.kind')!='message' OR EXISTS(SELECT 1 FROM messages m JOIN channels ch ON ch.id=m.channel_id WHERE m.id={alias}.message_id AND {message})) AND(json_extract({alias}.target,'$.kind')!='ask' OR EXISTS(SELECT 1 FROM object_refs r WHERE r.number={alias}.object_number AND {ask})))"
    );
    Ok((sql, values))
}

fn match_current(
    c: &Connection,
    d: &db::Document,
    query: &str,
) -> Result<Option<(MatchOn, String)>> {
    let needle = query.to_ascii_lowercase();
    for s in &d.slots {
        if s.text.to_ascii_lowercase().contains(&needle) {
            return Ok(Some((s.matched_on, clip(&s.text))));
        }
    }
    if let TargetRef::Table { id } = &d.target
        && let Some(v) =
            db::alias_match(c, crewd_store::managed_tables_v2::number(id, 'T')?, query)?
    {
        return Ok(Some((MatchOn::HistoricalName, clip(&v))));
    }
    Ok(None)
}
pub fn search(c: &Connection, a: Actor<'_>, mut q: Search, clock: ReadClock) -> Result<Page> {
    let now_ms = clock.wall_ms;
    a.principal(c)?;
    let kinds = validate(c, &mut q)?;
    let scope = fence(c, a, q.scope.as_ref())?;
    let token = q.cursor.take();
    let query = serde_json::to_string(&q)?;
    let mut cut = cursor(c, a, &query, token.as_deref(), &scope, &clock)?;
    if db::state(c)?.building {
        return Ok(Page {
            scope,
            index_state: IndexState::Building,
            snapshot: None,
            items: vec![],
            next_cursor: None,
        });
    }
    let (filter, args) = qualification(c, a, &q, &kinds, now_ms)?;
    let raw = db::candidates(
        c,
        cut.cut,
        cut.position.as_ref(),
        FACT_BUDGET,
        &filter,
        args,
    )?;
    let full = raw.len() == usize::from(FACT_BUDGET);
    let mut views = Views::new(c, a);
    let mut items = Vec::new();
    let mut remaining = false;
    for candidate in raw {
        if items.len() == usize::from(q.limit) {
            remaining = true;
            break;
        }
        cut.position = Some(candidate.position);
        let Some(target) = target_for_source(c, &candidate.source)? else {
            continue;
        };
        let current = match views.project(&target) {
            Ok(v) => v,
            Err(e) if hidden(&e) => continue,
            Err(e) => return Err(e),
        };
        if current.target != candidate.document.target {
            return Err(unavailable());
        }
        if !q.include_system_defaults && current.system_default {
            continue;
        }
        if q.crew_id
            .as_ref()
            .is_some_and(|crew| current.owner_crew.as_ref() != Some(crew))
            || q.table_id
                .as_ref()
                .is_some_and(|table| current.table_id.as_ref() != Some(table))
            || q.channel_id
                .as_ref()
                .is_some_and(|channel| current.channel_id.as_ref() != Some(channel))
        {
            continue;
        }

        let Some((matched_on, excerpt)) = match_current(c, &current, &q.query)? else {
            continue;
        };
        let table_title = current
            .table_id
            .as_ref()
            .map(|t| views.table(t).map(|t| t.title))
            .transpose()?;
        items.push(Hit {
            target: current.target,
            kind: current.kind.ok_or_else(unavailable)?,
            owner_crew_id: current.owner_crew,
            table_id: current.table_id,
            table_title,
            title: clip(&current.title),
            excerpt: Some(excerpt),
            matched_on,
            status: current.status,
            changed_at: current.changed_at,
        });
    }
    let next_cursor = if remaining || full {
        Some(encode(c, &cut)?)
    } else {
        None
    };
    Ok(Page {
        scope,
        index_state: IndexState::Ready,
        snapshot: Some(snapshot(&cut)),
        items,
        next_cursor,
    })
}

fn advance_graph(
    c: &Connection,
    views: &mut Views<'_>,
    span: Option<&str>,
    budget: u16,
) -> Result<usize> {
    let mut remaining = usize::from(budget);
    for (source, fingerprint, after, stamp) in db::graph_jobs_for(c, span, budget)? {
        if remaining == 0 {
            break;
        }
        let Some(target) = db::target(c, &source)? else {
            continue;
        };
        let current = views.project(&target)?;
        if db::fingerprint(&current)? != fingerprint {
            db::graph_job(c, &source, &db::fingerprint(&current)?)?;
            remaining -= 1;
            continue;
        }
        let pos: GraphPosition = after
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        if pos.stage == 2 {
            let (used, _) = db::graph_cleanup(c, &source, stamp, remaining as u16)?;
            remaining = remaining.saturating_sub(used.max(1));
            continue;
        }
        let (edges, next, raw) = graph_chunk(c, views, &current, &pos, remaining)?;
        remaining = remaining.saturating_sub(raw.max(1));
        for edge in &edges {
            if let Some(target_source) = source_key_for_target(&edge.target)? {
                db::ensure_source(c, &target_source, binding(&target_source), &edge.target)?;
            }
        }
        let done = next.is_none();
        let next = next.unwrap_or(GraphPosition {
            stage: 2,
            manual: 0,
            adapter: None,
        });
        db::graph_page(
            c,
            &source,
            stamp,
            &edges,
            Some(&serde_json::to_string(&next)?),
        )?;
        if done && remaining > 0 {
            let (used, _) = db::graph_cleanup(c, &source, stamp, remaining as u16)?;
            remaining = remaining.saturating_sub(used);
        }
    }
    Ok(usize::from(budget) - remaining)
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphPosition {
    stage: u8,
    manual: i64,
    adapter: Option<String>,
}
fn graph_chunk(
    c: &Connection,
    views: &mut Views<'_>,
    doc: &db::Document,
    pos: &GraphPosition,
    budget: usize,
) -> Result<(Vec<EdgeView>, Option<GraphPosition>, usize)> {
    let mut output = Vec::new();
    let mut raw = 0;
    let mut next = GraphPosition {
        stage: pos.stage,
        manual: pos.manual,
        adapter: pos.adapter.clone(),
    };
    if next.stage == 0 {
        // Artifact SourceRefs retain each real metadata owner; never invent one.
        let ids: Vec<i64> = if let TargetRef::Artifact { reference } = &doc.target {
            let mut q=c.prepare("SELECT id FROM managed_links WHERE source_artifact=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
            q.query_map(
                params![serde_json::to_string(reference)?, pos.manual, budget as i64],
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?
        } else if let Some(source) = source_ref(&doc.target, doc.owner_crew.as_deref()) {
            let mut q = c.prepare(
                "SELECT id FROM managed_links WHERE source_key=?1 AND id>?2 ORDER BY id LIMIT ?3",
            )?;
            q.query_map(
                params![
                    crewd_store::managed_links::source_key(&source)?,
                    pos.manual,
                    budget as i64
                ],
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?
        } else {
            vec![]
        };
        for id in &ids {
            raw += 1;
            next.manual = *id;
            if let Some(e) = crewd_store::managed_links::by_id(c, *id)? {
                match views.p.edge(&e) {
                    Ok(v) => output.push(v),
                    Err(e) if hidden(&e) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        if ids.len() == budget {
            return Ok((output, Some(next), raw));
        }
        next.stage = 1;
    }
    if raw < budget
        && let Some(source) = source_ref(&doc.target, doc.owner_crew.as_deref())
    {
        let pos = next
            .adapter
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        let chunk = views.p.adapter_chunk(&source, &pos, budget - raw)?;
        raw += chunk.raw_rows;
        output.extend(chunk.items.into_iter().map(|(_, v)| v));
        if let Some(p) = chunk.next {
            next.adapter = Some(serde_json::to_string(&p)?);
            return Ok((output, Some(next), raw));
        }
    }
    Ok((output, None, raw))
}

pub fn expand(
    c: &Connection,
    a: Actor<'_>,
    mut q: Expand,
    clock: ReadClock,
) -> Result<ExpansionPage> {
    let now_ms = clock.wall_ms;
    a.principal(c)?;
    if !(1..=MAX_LIMIT).contains(&q.limit)
        || matches!(
            q.target,
            TargetRef::Document { .. } | TargetRef::Knowledge { .. }
        )
    {
        return Err(invalid());
    }
    let scope = fence(c, a, q.scope.as_ref())?;
    let token = q.cursor.take();
    let query = serde_json::to_string(&q)?;
    let mut cut = cursor(c, a, &query, token.as_deref(), &scope, &clock)?;
    let mut views = Views::new(c, a);
    views.project(&q.target)?;
    if db::state(c)?.building {
        return Ok(ExpansionPage {
            scope,
            index_state: IndexState::Building,
            snapshot: None,
            items: vec![],
            next_cursor: None,
        });
    }
    let (source_filter, mut args) = endpoint_qualification(c, a, "src", 9, now_ms)?;
    let (neighbor_filter, next) = endpoint_qualification(c, a, "n", 9 + args.len(), now_ms)?;
    args.extend(next);
    let mut filter = format!("({source_filter}) AND ({neighbor_filter})");
    if !q.include_system_defaults {
        filter.push_str(" AND v.system_default=0");
    }
    let raw = db::ordered_graph(
        c,
        &q.target,
        cut.cut,
        cut.edge.as_ref(),
        FACT_BUDGET,
        &filter,
        args,
    )?;
    let full = raw.len() == usize::from(FACT_BUDGET);
    let mut rows = Vec::new();
    let mut remaining = false;
    for candidate in raw {
        let saved = candidate.edge;
        if rows.len() == usize::from(q.limit) {
            remaining = true;
            break;
        }
        cut.edge = Some(candidate.position);
        let source_target = source_target(&saved.source);
        let direction = if source_target == q.target {
            Direction::Outgoing
        } else {
            Direction::Incoming
        };
        let neighbor = if direction == Direction::Outgoing {
            saved.target.clone()
        } else {
            source_target
        };
        let current = match views.project(&neighbor) {
            Ok(v) => v,
            Err(e) if hidden(&e) => continue,
            Err(e) => return Err(e),
        };
        if !q.include_system_defaults && current.system_default {
            continue;
        }
        if let Err(e) = views.p.source(&saved.source, false) {
            if hidden(&e) { continue } else { return Err(e) }
        }
        let edge = match current_edge(c, &mut views, &saved) {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(e) if hidden(&e) => continue,
            Err(e) => return Err(e),
        };
        rows.push(Expansion {
            direction,
            edge,
            neighbor: views.p.resolve(&neighbor, ResolveMode::Read)?,
            status: current.status,
            changed_at: current.changed_at,
        });
    }
    Ok(ExpansionPage {
        scope,
        index_state: IndexState::Ready,
        snapshot: Some(snapshot(&cut)),
        items: rows,
        next_cursor: if remaining || full {
            Some(encode(c, &cut)?)
        } else {
            None
        },
    })
}
fn current_edge(
    c: &Connection,
    views: &mut Views<'_>,
    saved: &EdgeView,
) -> Result<Option<EdgeView>> {
    match &saved.origin {
        EdgeOrigin::ManualRelates {} | EdgeOrigin::Column { .. } => {
            let Some(e) =
                crewd_store::managed_links::by_id(c, saved.id.parse().map_err(|_| unavailable())?)?
            else {
                return Ok(None);
            };
            if e.source != saved.source || e.target != saved.target {
                return Err(unavailable());
            }
            Ok(Some(views.p.edge(&e)?))
        }
        EdgeOrigin::Adapter { .. } => views.p.exact_adapter(saved),
    }
}

#[cfg(test)]
#[path = "organization_search_tests.rs"]
mod tests;
