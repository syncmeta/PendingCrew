//! Same-crew suggestions over original ordinary AgentAsk questions.
//! Neither observations nor reads answer, merge, withdraw or re-time an Ask.
#[cfg(test)]
#[path = "admin_ask_groups_tests.rs"]
mod tests;
use crate::{Result, Sender, Tx, asks, channels, crews, judgments, resource_acl, todo_attributes};
use crewd_proto::{
    ChannelKind, ChannelWorkKind, ChannelWorkObject, ManualTodoState, TodoOrigin,
    admin_ask_groups::*,
};
use crewd_store::{
    StoreError, admin_ask_groups as db,
    object_refs::{self, ObjectKind},
};
use rusqlite::{Connection, OptionalExtension, params};

const PEERS: i64 = 8;
const PENDING: u16 = 32;
const TEXT_BYTES: usize = 1_024;
const DEFAULT_BYTES: usize = 512;
const PAGE_BYTES: usize = 24 * 1_024;
pub const SUGGESTION_MIN: f64 = 0.8;

fn invalid(field: &str) -> crate::CollabError {
    resource_acl::invalid(field)
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn excerpt(value: &str, bytes: usize) -> (String, bool) {
    let end = value.floor_char_boundary(bytes.min(value.len()));
    (value[..end].into(), end < value.len())
}

fn configured(c: &Connection) -> Result<Option<String>> {
    let settings = judgments::configuration(c)?;
    if settings.revision.is_none()
        || settings
            .config
            .as_ref()
            .is_none_or(|v| !v.enabled || v.similar_asks != Some(true))
        || crewd_store::meta_get(c, judgments::PENDING_KEY)?.is_some()
    {
        return Ok(None);
    }
    Ok(Some(judgments::fingerprint(&settings)?))
}

fn scope(c: &Connection) -> Result<crewd_proto::managed_objects::Scope> {
    crate::managed_objects::scope(c)
}

#[derive(Clone)]
pub struct Snapshot {
    pub id: String,
    pub crew: String,
    pub first: Question,
    pub second: Question,
    pub fingerprint: String,
    pub authority_fingerprint: String,
    pub excerpt_truncated: bool,
}

#[derive(Clone, Copy)]
pub enum Outcome {
    Answered { probability: f64, suggested: bool },
    Unavailable,
    Unknown,
}

struct Source {
    question: Question,
    fingerprint: String,
    authority_fingerprint: String,
}

enum Probe {
    Excluded,
    Unavailable,
    Ready(Box<Source>),
}

fn probe(c: &Connection, number: i64, now: i64) -> Result<Probe> {
    let Some(reference) = object_refs::get(c, ObjectKind::HumanTask, number)? else {
        return Ok(Probe::Excluded);
    };
    if crews::is_archived(c, &reference.crew_id)? {
        return Ok(Probe::Excluded);
    }
    // Structured decision questions have their own fixed options and capture semantics.
    if crewd_store::decision_asks::read_ask(c, number)?.is_some() {
        return Ok(Probe::Excluded);
    }
    let subject = asks::resolution_subject(c, &reference.crew_id, &reference.id(), None)?;
    let Some(revision) = subject.revision else {
        return Ok(Probe::Excluded);
    };
    let attrs = todo_attributes::get(
        c,
        &Sender::Human,
        &ChannelWorkObject {
            kind: ChannelWorkKind::HumanTask,
            id: reference.id(),
        },
    )?;
    if attrs.origin != TodoOrigin::AgentAsk || attrs.manual_state == Some(ManualTodoState::Done) {
        return Ok(Probe::Excluded);
    }
    let book = asks::load_org(c)?;
    let Some(original) = book.ask(subject.id) else {
        return Ok(Probe::Excluded);
    };
    if original.human_task || original.state != crewd_work::AskState::Open {
        return Ok(Probe::Excluded);
    }
    let default_due = original
        .default
        .as_ref()
        .map(|d| original.asked_at.0.saturating_add(d.after_ms));
    if default_due.is_some_and(|due| due <= now.max(book.state().clock.0)) {
        return Ok(Probe::Excluded);
    }
    let home = crewd_store::channels::human_id(&crews::home_of(c, &reference.crew_id)?);
    let channel = match channels::get(c, &home) {
        Ok(channel) => channel,
        Err(crate::CollabError::NotFound(_)) => return Ok(Probe::Unavailable),
        Err(error) => return Err(error),
    };
    if channel.kind != ChannelKind::Human || channel.archived_at.is_some() {
        return Ok(Probe::Unavailable);
    }
    // Bind the actual announcement, not a guessed public Topic, alias, or client excerpt.
    let message:Option<String>=c.query_row("SELECT m.id FROM ask_meta a JOIN messages m ON m.id=a.message_id WHERE a.crew_id=?1 AND a.ask=?2 AND m.channel_id=?3 AND m.category='ask' AND m.sender_session_id IS ?4 AND m.text=?5",params![reference.crew_id,asks::meta_key(&reference),home,subject.view.asked_by,subject.view.question],|r|r.get(0)).optional()?;
    let Some(message) = message else {
        return Ok(Probe::Unavailable);
    };
    let crew = crews::get(c, &reference.crew_id)?;
    let stop = match subject.view.asked_by.as_deref() {
        Some(s) => Some((
            crate::wakes::human_stop_floor(c, s)?,
            judgments::stop_epoch(c, s)?,
        )),
        None => None,
    };
    // Model intent is separate from transient credential, cancellation and CAS
    // evidence. Reconfiguration must not re-enqueue an uncertain original pair.
    let fingerprint = judgments::fingerprint(&(
        &reference.source_id,
        &reference.created_at,
        &crew.created_at,
        &original.question,
        &original.default,
        &original.asked_by,
        original.asked_at,
        original.item,
        original.standalone,
    ))?;
    let authority_fingerprint = judgments::fingerprint(&(
        &fingerprint,
        &revision,
        &home,
        &message,
        &subject.view,
        &attrs,
        &stop,
    ))?;
    let (question, question_truncated) = excerpt(&subject.view.question, TEXT_BYTES);
    let (default, default_truncated) = match subject.view.default.as_ref() {
        Some(d) => {
            let (v, t) = excerpt(&d.text, DEFAULT_BYTES);
            (Some(v), t)
        }
        None => (None, false),
    };
    Ok(Probe::Ready(Box::new(Source {
        fingerprint,
        authority_fingerprint,
        question: Question {
            id: reference.id(),
            question,
            question_truncated,
            resolution_revision: revision,
            source_channel_id: home,
            source_message_id: message,
            asked_at_ms: original.asked_at.0,
            default,
            default_due_at_ms: default_due,
            default_truncated,
        },
    })))
}

fn source(c: &Connection, number: i64, now: i64) -> Result<Option<Source>> {
    Ok(match probe(c, number, now)? {
        Probe::Ready(s) => Some(*s),
        _ => None,
    })
}
fn observed_source(
    c: &Connection,
    number: i64,
    crew: &str,
    scope: &crewd_proto::managed_objects::Scope,
    config: &str,
    now: i64,
) -> Result<Option<Source>> {
    let p = probe(c, number, now)?;
    let (eligibility, fingerprint) = match &p {
        Probe::Excluded => ("excluded", None),
        Probe::Unavailable => ("unavailable", None),
        Probe::Ready(s) => ("eligible", Some(s.fingerprint.as_str())),
    };
    db::note_source(c, number, crew, scope, config, eligibility, fingerprint)?;
    Ok(match p {
        Probe::Ready(s) => Some(*s),
        _ => None,
    })
}

fn snapshot(c: &Connection, row: &db::Observation, now: i64) -> Result<Option<Snapshot>> {
    if !hex(&row.id) || !hex(&row.fingerprint) || row.first <= 0 || row.second <= row.first {
        return Err(invalid("observation"));
    }
    let recorded_scope = crewd_proto::managed_objects::Scope {
        db_id: row.db.clone(),
        restore_epoch: row.epoch.clone(),
    };
    let recorded = judgments::fingerprint(&(
        &recorded_scope,
        &row.crew,
        row.first,
        row.second,
        &row.config,
        &row.first_fingerprint,
        &row.second_fingerprint,
    ))?;
    if !hex(&row.first_fingerprint)
        || !hex(&row.second_fingerprint)
        || recorded != row.fingerprint
        || judgments::fingerprint(&("admin-ask-groups-v1", &recorded))? != row.id
    {
        return Err(invalid("observation"));
    }
    let coherent = match row.phase.as_str() {
        "prepared" | "dispatched" => row.probability.is_none() && row.observed_at.is_none(),
        "suggested" | "different" => {
            row.observed_at.is_some()
                && row.probability.is_some_and(|v| {
                    v.is_finite()
                        && (0.0..=1.0).contains(&v)
                        && (v >= SUGGESTION_MIN) == (row.phase == "suggested")
                })
        }
        "unavailable" | "unknown" | "stale" => row.probability.is_none(),
        _ => false,
    };
    if !coherent {
        return Err(invalid("observation"));
    }
    let current = scope(c)?;
    if row.db != current.db_id
        || row.epoch != current.restore_epoch
        || configured(c)?.as_deref() != Some(&row.config)
    {
        return Ok(None);
    }
    let (Some(a), Some(b)) = (source(c, row.first, now)?, source(c, row.second, now)?) else {
        return Ok(None);
    };
    if row.first_fingerprint != a.fingerprint || row.second_fingerprint != b.fingerprint {
        return Ok(None);
    }
    let first =
        object_refs::get(c, ObjectKind::HumanTask, row.first)?.ok_or_else(|| invalid("first"))?;
    let second =
        object_refs::get(c, ObjectKind::HumanTask, row.second)?.ok_or_else(|| invalid("second"))?;
    if first.crew_id != row.crew || second.crew_id != row.crew {
        return Ok(None);
    }
    let fingerprint = judgments::fingerprint(&(
        &current,
        &row.crew,
        row.first,
        row.second,
        &row.config,
        &a.fingerprint,
        &b.fingerprint,
    ))?;
    if fingerprint != row.fingerprint
        || judgments::fingerprint(&("admin-ask-groups-v1", &fingerprint))? != row.id
    {
        return Ok(None);
    }
    Ok(Some(Snapshot {
        id: row.id.clone(),
        crew: row.crew.clone(),
        excerpt_truncated: a.question.question_truncated
            || a.question.default_truncated
            || b.question.question_truncated
            || b.question.default_truncated,
        first: a.question,
        second: b.question,
        fingerprint,
        authority_fingerprint: judgments::fingerprint(&(
            &a.authority_fingerprint,
            &b.authority_fingerprint,
        ))?,
    }))
}

/// Restores never redispatch requests whose external response was not observed.
/// The restored cut is retained even while the new semantic family is disabled.
pub fn recover_page(tx: &mut Tx<'_>) -> Result<usize> {
    Ok(db::recover(tx.conn(), PENDING)?)
}

/// Root's single collaboration transaction calls this predicate on its actual
/// committed change set. Advice itself is deliberately not a source mutation.
pub fn note_source_changes(tx: &mut Tx<'_>, changes: &[crate::Change]) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let current = scope(tx.conn())?;
    let config = judgments::fingerprint(&judgments::configuration(tx.conn())?)?;
    let mut dirty = false;
    for change in changes {
        match change {
            crate::Change::AskChanged { crew_id, ask } => {
                let Some(number) = ask.strip_prefix('H').and_then(|n| n.parse::<i64>().ok()) else {
                    dirty = true;
                    continue;
                };
                let Some(reference) = object_refs::get(tx.conn(), ObjectKind::HumanTask, number)?
                else {
                    continue;
                };
                if reference.crew_id != *crew_id {
                    return Err(invalid("sourceCrew"));
                }
                let previous = db::pass_cut(tx.conn(), crew_id)?;
                db::enroll_cursor(tx.conn(), crew_id, &current, &config)?;
                if previous.is_some_and(|cut| number <= cut) {
                    dirty = true;
                }
            }
            crate::Change::TodoAttributesChanged {
                crew_id, object, ..
            } if object.kind == ChannelWorkKind::HumanTask => {
                let number = object
                    .id
                    .strip_prefix('H')
                    .and_then(|n| n.parse::<i64>().ok());
                let previous = db::pass_cut(tx.conn(), crew_id)?;
                if number.zip(previous).is_none_or(|(n, cut)| n <= cut) {
                    dirty = true;
                }
            }
            crate::Change::CrewChanged { .. }
            | crate::Change::CrewRemoved { .. }
            | crate::Change::ChannelChanged { .. }
            | crate::Change::ChannelRemoved { .. }
            | crate::Change::SessionChanged { .. } => dirty = true,
            _ => {}
        }
    }
    if dirty {
        db::invalidate_coverage(tx.conn())?;
    }
    Ok(())
}

/// Bounded restore reconciliation: the caller repeats pages before enabling the
/// runtime. Current rows do not rewrite metadata, even with this family disabled.
pub fn reconcile_cursor(tx: &mut Tx<'_>) -> Result<usize> {
    let current = scope(tx.conn())?;
    let rows = db::stale_cursors(tx.conn(), &current, 64)?;
    let count = rows.len();
    let config = judgments::fingerprint(&judgments::configuration(tx.conn())?)?;
    for old in rows {
        let floor = db::cut(tx.conn(), &old.crew)?;
        let generation = uuid::Uuid::new_v4().to_string();
        db::save_cursor(
            tx.conn(),
            &db::Cursor {
                crew: old.crew,
                db: current.db_id.clone(),
                epoch: current.restore_epoch.clone(),
                config: config.clone(),
                floor,
                anchor: floor,
                peer: 0,
                completed_cut: floor,
                pass_cut: floor,
                generation: generation.clone(),
                pass_generation: generation,
            },
        )?;
    }
    Ok(count)
}

/// One advancing anchor and at most eight source peers per tick. Every earlier
/// published H in that crew is eventually visited. Prepare at most ONE actual
/// judgment, rather than materializing a future N-squared dispatch queue.
pub fn observe_page(tx: &mut Tx<'_>) -> Result<()> {
    let Some(config) = configured(tx.conn())? else {
        return Ok(());
    };
    reconcile_cursor(tx)?;
    let current = scope(tx.conn())?;
    let Some(crew) = db::next_crew(tx.conn())? else {
        return Ok(());
    };
    let mut cursor = db::ensure_cursor(tx.conn(), &crew, &current, &config)?;
    // Startup normally reconciles every old cursor. If recovery was interrupted,
    // a later bounded pass must not enroll old work under the new restore scope.
    if cursor.db != current.db_id || cursor.epoch != current.restore_epoch {
        return Ok(());
    }
    if cursor.config != config {
        cursor.config = config.clone();
        // A new explicit settings UUID is a fresh opt-in, not an automatic
        // continuation of the restored epoch.
        cursor.floor = 0;
        cursor.anchor = 0;
        cursor.peer = 0;
        cursor.completed_cut = 0;
        cursor.pass_cut = db::cut(tx.conn(), &crew)?;
        cursor.pass_generation = cursor.generation.clone();
    }
    let anchor:Option<(i64,String)>=tx.conn().query_row("SELECT number,crew_id FROM object_refs WHERE kind='human_task' AND crew_id=?3 AND number>?1 AND number<=?2 ORDER BY number LIMIT 1",params![cursor.anchor,cursor.pass_cut,crew],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((second, crew)) = anchor else {
        let was_complete =
            cursor.completed_cut >= cursor.pass_cut && cursor.generation == cursor.pass_generation;
        if cursor.generation == cursor.pass_generation {
            cursor.completed_cut = cursor.pass_cut;
            cursor.anchor = cursor.pass_cut;
        } else {
            cursor.completed_cut = cursor.floor;
            cursor.anchor = cursor.floor;
        }
        cursor.pass_generation = cursor.generation.clone();
        let cut = db::cut(tx.conn(), &crew)?;
        cursor.pass_cut = cut.max(cursor.floor);
        cursor.peer = 0;
        db::save_cursor(tx.conn(), &cursor)?;
        if !was_complete
            && cursor.generation == cursor.pass_generation
            && cursor.completed_cut >= cursor.pass_cut
        {
            tx.note(crate::Change::AdminAskGroupsChanged {
                crew_id: crew.clone(),
            });
        }
        return Ok(());
    };
    let now = tx.now_ms();
    let Some(b) = observed_source(tx.conn(), second, &crew, &current, &config, now)? else {
        cursor.anchor = second;
        cursor.peer = 0;
        db::save_cursor(tx.conn(), &cursor)?;
        return Ok(());
    };
    let mut peers = {
        let mut q=tx.conn().prepare("SELECT number FROM object_refs WHERE kind='human_task' AND crew_id=?1 AND number>?2 AND number<?3 ORDER BY number LIMIT ?4")?;
        q.query_map(
            params![crew, cursor.peer.max(cursor.floor), second, PEERS + 1],
            |r| r.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let more = peers.len() > PEERS as usize;
    if more {
        peers.pop();
    }
    for first in peers.iter().copied() {
        let Some(a) = observed_source(tx.conn(), first, &crew, &current, &config, now)? else {
            continue;
        };
        let fingerprint = judgments::fingerprint(&(
            &current,
            &crew,
            first,
            second,
            &config,
            &a.fingerprint,
            &b.fingerprint,
        ))?;
        let id = judgments::fingerprint(&("admin-ask-groups-v1", &fingerprint))?;
        let added = db::prepare(
            tx.conn(),
            &db::Observation {
                id,
                crew: crew.clone(),
                first,
                second,
                db: current.db_id.clone(),
                epoch: current.restore_epoch.clone(),
                config: config.clone(),
                fingerprint,
                first_fingerprint: a.fingerprint,
                second_fingerprint: b.fingerprint.clone(),
                phase: "prepared".into(),
                probability: None,
                observed_at: None,
            },
        )?;
        if added {
            cursor.peer = first;
            db::save_cursor(tx.conn(), &cursor)?;
            return Ok(());
        }
    }
    if more {
        cursor.peer = *peers.last().ok_or_else(|| invalid("cursor"))?;
    } else {
        cursor.anchor = second;
        cursor.peer = 0;
    }
    db::save_cursor(tx.conn(), &cursor)?;
    Ok(())
}

pub fn next_dispatch(tx: &mut Tx<'_>) -> Result<Option<Snapshot>> {
    if configured(tx.conn())?.is_none() {
        return Ok(None);
    }
    for row in db::pending(tx.conn(), PENDING)? {
        let next = snapshot(tx.conn(), &row, tx.now_ms())?;
        if next.is_some() {
            if db::transition(tx.conn(), &row.id, "prepared", "dispatched", None, None)? {
                return Ok(next);
            }
        } else {
            db::transition(tx.conn(), &row.id, "prepared", "stale", None, None)?;
        }
    }
    Ok(None)
}

pub fn still_current(c: &Connection, s: &Snapshot, now: i64) -> Result<bool> {
    let Some(row) = db::get(c, &s.id)? else {
        return Ok(false);
    };
    Ok(row.phase == "dispatched"
        && snapshot(c, &row, now)?.is_some_and(|current| {
            current.fingerprint == s.fingerprint
                && current.authority_fingerprint == s.authority_fingerprint
        }))
}

pub fn settle(tx: &mut Tx<'_>, s: &Snapshot, outcome: Outcome) -> Result<bool> {
    let Some(row) = db::get(tx.conn(), &s.id)? else {
        return Ok(false);
    };
    if row.phase != "dispatched" || row.fingerprint != s.fingerprint {
        return Ok(false);
    }
    let (phase, probability) = if snapshot(tx.conn(), &row, tx.now_ms())?
        .is_none_or(|current| current.authority_fingerprint != s.authority_fingerprint)
    {
        ("stale", None)
    } else {
        match outcome {
            Outcome::Answered {
                probability,
                suggested,
            } if probability.is_finite()
                && (0.0..=1.0).contains(&probability)
                && suggested == (probability >= SUGGESTION_MIN) =>
            {
                (
                    if suggested { "suggested" } else { "different" },
                    Some(probability),
                )
            }
            Outcome::Unknown => ("unknown", None),
            _ => ("unavailable", None),
        }
    };
    Ok(db::transition(
        tx.conn(),
        &s.id,
        "dispatched",
        phase,
        probability,
        Some(tx.now_ms()),
    )?)
}

pub fn list(c: &Connection, by: &Sender, p: List, now: i64) -> Result<Page> {
    resource_acl::actor(c, by, &p.crew_id, false)?;
    // This Human surface must not become a new Agent read or execution grant.
    if !matches!(by, Sender::Human) {
        return Err(crate::CollabError::Forbidden(crate::refusal::Refusal::new(
            crate::refusal::Code::ResourceForbidden,
        )));
    }
    let limit = p.limit.unwrap_or(5);
    if !(1..=20).contains(&limit) {
        return Err(invalid("limit"));
    }
    let current = scope(c)?;
    let mut page = Page {
        scope: current.clone(),
        crew_id: p.crew_id.clone(),
        availability: Availability::Ready,
        observed_at_ms: now,
        items: Vec::new(),
        next_after: None,
        partial_coverage: true,
        historical_baseline: false,
    };
    if crews::is_archived(c, &p.crew_id)? {
        page.availability = Availability::Archived;
        return Ok(page);
    }
    let settings = judgments::configuration(c)?;
    if settings
        .config
        .as_ref()
        .is_none_or(|v| !v.enabled || v.similar_asks != Some(true))
    {
        page.availability = Availability::Disabled;
        return Ok(page);
    }
    let Some(config) = configured(c)? else {
        page.availability = Availability::Unconfigured;
        return Ok(page);
    };
    if crewd_store::meta_get(c, judgments::ACTIVE_KEY)?.is_none() {
        page.availability = Availability::Unconfigured;
        return Ok(page);
    }
    let cut = db::cut(c, &p.crew_id)?;
    if let Some(scan) = db::cursor(c, &p.crew_id)? {
        page.historical_baseline =
            scan.floor > 0 || scan.db != current.db_id || scan.epoch != current.restore_epoch;
        page.partial_coverage = scan.db != current.db_id
            || scan.epoch != current.restore_epoch
            || scan.config != config
            || scan.generation != scan.pass_generation
            || scan.completed_cut < cut
            || scan.floor > 0
            || db::has_gaps(c, &p.crew_id, &current, &config)?
            || db::unresolved(c, &p.crew_id, &current, &config)?;
    } else {
        page.partial_coverage = cut > 0;
    }
    let witness = judgments::fingerprint(&(&current, &p.crew_id, &config))?;
    let after = p
        .after
        .as_deref()
        .map(|text| {
            let (w, id) = text.split_once(':').ok_or_else(|| invalid("after"))?;
            if w != witness || !hex(id) {
                return Err(invalid("after"));
            }
            Ok(id)
        })
        .transpose()?;
    let (rows, next) = db::page(c, &p.crew_id, after, limit)?;
    let mut last = after.map(str::to_string);
    for row in rows {
        if let Some(s) = snapshot(c, &row, now)? {
            let state = match row.phase.as_str() {
                "prepared" => Some(State::Prepared),
                "dispatched" => Some(State::Dispatched),
                "suggested" => Some(State::Suggested),
                "different" => Some(State::Different),
                "unavailable" => Some(State::Unavailable),
                "unknown" => Some(State::Unknown),
                "stale" => Some(State::Stale),
                _ => None,
            };
            if let Some(state) = state {
                if matches!(state, State::Suggested | State::Different)
                    && row
                        .probability
                        .is_none_or(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
                {
                    return Err(invalid("observation"));
                }
                let pair = Pair {
                    id: row.id.clone(),
                    first: s.first,
                    second: s.second,
                    state,
                    probability: row.probability,
                    observed_at_ms: row.observed_at,
                    excerpt_truncated: s.excerpt_truncated,
                };
                let bytes = serde_json::to_vec(&pair).map_err(StoreError::from)?.len();
                let used = serde_json::to_vec(&page).map_err(StoreError::from)?.len();
                if used + bytes + 512 > PAGE_BYTES {
                    let Some(last) = last else {
                        return Err(invalid("pageBytes"));
                    };
                    page.next_after = Some(format!("{witness}:{last}"));
                    return Ok(page);
                }
                page.items.push(pair);
            }
        }
        last = Some(row.id);
    }
    page.next_after = next.map(|id| format!("{witness}:{id}"));
    Ok(page)
}
