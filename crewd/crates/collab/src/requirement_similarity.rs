//! Optional suggestions over two original Human requirements. Only observation
//! metadata is written here. A suggestion cannot alter either requirement.
use crate::{Result, Sender, Tx, asks, channels, crews, judgments, todo_attributes, wakes};
use crewd_proto::{
    ChannelKind, ChannelWorkKind, ChannelWorkObject, TodoOrigin,
    admin_cases::{SimilarRequirement, SimilarRequirementsPage},
};
use crewd_store::{StoreError, judgment_state, meta_get, meta_set, object_refs::ObjectKind};
use crewd_work::{EntryNo, WorkBook};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const RECORD: &str = "requirement.similarity.record.v1:";
const PENDING: &str = "requirement.similarity.pending.v1:";
const QUARANTINE: &str = "requirement.similarity.quarantine.v1:";
const CURSOR: &str = "requirement.similarity.cursor.v1";
const ANCHOR_PAGE: i64 = 8;
const NEARBY_IDS: i64 = 128;
const CANDIDATE_LIMIT: i64 = 8;
const MAX_EXCERPT_CHARS: usize = 1_200;
const PENDING_PAGE: u16 = 64;
type KeysetPage<T> = (Vec<(String, T)>, Option<String>);

#[derive(Clone)]
pub struct Snapshot {
    pub id: String,
    pub crew: String,
    pub first_id: String,
    pub second_id: String,
    pub first_text: String,
    pub second_text: String,
    pub excerpt_truncated: bool,
    pub fingerprint: String,
    pub(crate) channel: String,
    pub(crate) review_witness: String,
}

impl Snapshot {
    pub(crate) fn candidate_revision(&self) -> Result<String> {
        judgments::fingerprint(&(&self.fingerprint, &self.review_witness))
    }
}

pub enum Outcome {
    Answered { probability: f64, suggested: bool },
    Unavailable,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    epoch: String,
    number: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum State {
    Prepared,
    Dispatched,
    Answered {
        probability: f64,
        suggested: bool,
        at: i64,
    },
    Unavailable,
    Unknown,
    Stale,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    crew: String,
    first: i64,
    second: i64,
    fingerprint: String,
    state: State,
}

struct Authority {
    epoch: String,
    config_fingerprint: String,
    book: WorkBook,
    claimed: BTreeSet<EntryNo>,
}

struct Source {
    crew: String,
    channel: String,
    excerpt: String,
    excerpt_truncated: bool,
    fingerprint: String,
    review_fingerprint: String,
}

fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid requirement similarity authority".into()).into()
}

fn config(c: &Connection) -> Result<Option<String>> {
    let settings = judgments::configuration(c)?;
    if settings.revision.is_none()
        || settings
            .config
            .as_ref()
            .is_none_or(|v| !v.enabled || v.similar_requirements != Some(true))
        || meta_get(c, judgments::PENDING_KEY)?.is_some()
    {
        return Ok(None);
    }
    Ok(Some(judgments::fingerprint(&settings)?))
}

impl Authority {
    fn load(c: &Connection, config_fingerprint: String) -> Result<Self> {
        let book = asks::load_org(c)?;
        let claimed = book
            .state()
            .items
            .values()
            .flat_map(|item| item.sources.iter().copied())
            .collect();
        Ok(Self {
            epoch: judgment_state::epoch(c)?,
            config_fingerprint,
            book,
            claimed,
        })
    }

    fn source(&self, c: &Connection, number: i64) -> Result<Option<Source>> {
        self.source_for(c, number, true)
    }

    fn source_for(&self, c: &Connection, number: i64, scoring: bool) -> Result<Option<Source>> {
        let Some(reference) = crewd_store::object_refs::get(c, ObjectKind::Requirement, number)?
        else {
            return Ok(None);
        };
        let raw = c
            .query_row(
                "SELECT t.id,t.text,t.extra,t.created_at FROM object_refs r JOIN todos t
             ON t.id=r.raw_todo_id AND t.crew_id=r.crew_id
             WHERE r.number=?1 AND t.ledger='agent' AND t.deleted_at IS NULL",
                [number],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((raw_id, text, Some(extra), created_at)) = raw else {
            return Ok(None);
        };
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Original {
            requirement_request_id: String,
            source_channel_id: String,
        }
        let Ok(original) = serde_json::from_str::<Original>(&extra) else {
            return Ok(None);
        };
        let Ok(request) = uuid::Uuid::parse_str(&original.requirement_request_id) else {
            return Ok(None);
        };
        if raw_id != format!("requirement-request-{request}")
            || text.trim().is_empty()
            || crews::is_archived(c, &reference.crew_id)?
        {
            return Ok(None);
        }
        let core: EntryNo =
            crewd_store::work_organization::core_for_reference(c, ObjectKind::Requirement, number)?
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
        let entry = self.book.entry(core).ok_or_else(invalid)?;
        if self.book.state().entry_crew(core).map(|id| id.as_str())
            != Some(reference.crew_id.as_str())
        {
            return Err(invalid());
        }
        if scoring && (entry.closed.is_some() || self.claimed.contains(&core)) {
            return Ok(None);
        }
        let object = ChannelWorkObject {
            kind: ChannelWorkKind::Requirement,
            id: reference.id(),
        };
        let attributes = todo_attributes::get(c, &Sender::Human, &object)?;
        if attributes.origin != TodoOrigin::Requirement
            || (scoring
                && (attributes.owner.is_some()
                    || attributes.manual_state == Some(crewd_proto::ManualTodoState::Done)))
        {
            return Ok(None);
        }
        let channel = match channels::get(c, &original.source_channel_id) {
            Ok(channel) => channel,
            Err(crate::CollabError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        if channel.crew_id != reference.crew_id
            || channel.archived_at.is_some()
            || !matches!(channel.kind, ChannelKind::Lobby | ChannelKind::Topic)
            || !crewd_store::channel_work::linked(c, &channel.id, &object)?
        {
            return Ok(None);
        }
        // Bind the last event for this link, not the channel's shared revision.
        // Unlink/relink invalidates an answer while unrelated new work does not.
        let linkage = c
            .query_row(
                "SELECT revision,json_type(body,'$.linked') FROM channel_work_events
             WHERE channel_id=?1 AND json_extract(body,'$.object.kind')='requirement'
               AND json_extract(body,'$.object.id')=?2
               AND json_type(body,'$.linked') IN ('true','false')
             ORDER BY revision DESC LIMIT 1",
                params![channel.id, object.id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((link_revision, linked)) = linkage else {
            return Ok(None);
        };
        if link_revision <= 0 || linked != "true" {
            return Ok(None);
        }
        let generation = crate::channel_archive::generation(c, &channel.id)?;
        let review_fingerprint = judgments::fingerprint(&(
            number,
            &reference.crew_id,
            core,
            &raw_id,
            &created_at,
            &text,
            &channel.id,
            link_revision,
            &generation,
        ))?;
        let fingerprint = if scoring {
            judgments::fingerprint(&(
                number,
                &reference.crew_id,
                core,
                raw_id,
                created_at,
                &text,
                &channel,
                attributes.revision,
                link_revision,
            ))?
        } else {
            String::new()
        };
        let mut chars = text.chars();
        let excerpt = if scoring {
            chars.by_ref().take(MAX_EXCERPT_CHARS).collect()
        } else {
            String::new()
        };
        Ok(Some(Source {
            crew: reference.crew_id,
            channel: channel.id,
            excerpt,
            excerpt_truncated: scoring && chars.next().is_some(),
            fingerprint,
            review_fingerprint,
        }))
    }

    fn pair(
        &self,
        c: &Connection,
        first: i64,
        a: Source,
        second: i64,
        b: Source,
    ) -> Result<Option<Snapshot>> {
        if first >= second || a.crew != b.crew || a.channel != b.channel {
            return Ok(None);
        }
        let stop = match crews::captain_of(c, &a.crew)? {
            Some(id) => {
                if wakes::automatic_work_paused(c, &id)? {
                    return Ok(None);
                }
                let floor = wakes::human_stop_floor(c, &id)?;
                let epoch = judgments::stop_epoch(c, &id)?;
                Some((id, floor, epoch))
            }
            None => None,
        };
        let id = pair_id(&a.crew, first, second)?;
        let fingerprint = judgments::fingerprint(&(
            &id,
            &a.fingerprint,
            &b.fingerprint,
            &self.epoch,
            &self.config_fingerprint,
            stop,
        ))?;
        let review_witness = self.review_witness(&a, &b)?;
        Ok(Some(Snapshot {
            id,
            crew: a.crew,
            first_id: format!("#{first}"),
            second_id: format!("#{second}"),
            first_text: a.excerpt,
            second_text: b.excerpt,
            excerpt_truncated: a.excerpt_truncated || b.excerpt_truncated,
            fingerprint,
            channel: a.channel,
            review_witness,
        }))
    }

    fn review_witness(&self, a: &Source, b: &Source) -> Result<String> {
        judgments::fingerprint(&(&self.epoch, &a.review_fingerprint, &b.review_fingerprint))
    }

    fn snapshot(&self, c: &Connection, record: &Record) -> Result<Option<Snapshot>> {
        let (Some(a), Some(b)) = (
            self.source(c, record.first)?,
            self.source(c, record.second)?,
        ) else {
            return Ok(None);
        };
        self.pair(c, record.first, a, record.second, b)
    }
}

/// Review visibility uses the same immutable source and link qualification
/// without depending on model configuration or later Work ownership.
pub(crate) struct ReviewSources(Authority);

impl ReviewSources {
    pub fn load(c: &Connection) -> Result<Self> {
        Ok(Self(Authority::load(c, String::new())?))
    }

    pub fn witness(
        &self,
        c: &Connection,
        crew: &str,
        channel: &str,
        first: i64,
        second: i64,
    ) -> Result<Option<String>> {
        let a = self.0.source_for(c, first, false)?;
        let b = self.0.source_for(c, second, false)?;
        let (Some(a), Some(b)) = (a, b) else {
            return Ok(None);
        };
        if a.crew != crew || b.crew != crew || a.channel != channel || b.channel != channel {
            return Ok(None);
        }
        Ok(Some(self.0.review_witness(&a, &b)?))
    }
}

pub(crate) fn current_suggestion(
    c: &Connection,
    crew: &str,
    first: i64,
    second: i64,
) -> Result<Option<Snapshot>> {
    let Some(config) = config(c)? else {
        return Ok(None);
    };
    let id = pair_id(crew, first, second)?;
    let Some(record) = load(c, &id)? else {
        return Ok(None);
    };
    if !matches!(
        record.state,
        State::Answered {
            suggested: true,
            ..
        }
    ) || quarantined(c, &id)?
    {
        return Ok(None);
    }
    let snapshot = Authority::load(c, config)?.snapshot(c, &record)?;
    Ok(snapshot.filter(|snapshot| snapshot.fingerprint == record.fingerprint))
}

fn pair_id(crew: &str, first: i64, second: i64) -> Result<String> {
    Ok(format!(
        "{}:{first:020}:{second:020}",
        judgments::fingerprint(&crew)?
    ))
}

fn decode(id: &str, raw: &str) -> Result<Record> {
    let record: Record = serde_json::from_str(raw).map_err(StoreError::from)?;
    if record.version != 1
        || record.first <= 0
        || record.first >= record.second
        || record.fingerprint.len() != 64
        || pair_id(&record.crew, record.first, record.second)? != id
        || matches!(record.state, State::Answered { probability, .. } if !(0.0..=1.0).contains(&probability))
    {
        return Err(invalid());
    }
    Ok(record)
}

fn load(c: &Connection, id: &str) -> Result<Option<Record>> {
    meta_get(c, &format!("{RECORD}{id}"))?
        .map(|raw| decode(id, &raw))
        .transpose()
}

fn save(tx: &mut Tx<'_>, id: &str, record: &Record) -> Result<()> {
    meta_set(
        tx.conn(),
        &format!("{RECORD}{id}"),
        &serde_json::to_string(record).map_err(StoreError::from)?,
    )?;
    let pending = format!("{PENDING}{id}");
    if matches!(record.state, State::Prepared | State::Dispatched) {
        meta_set(tx.conn(), &pending, id)?;
    } else {
        tx.conn()
            .execute("DELETE FROM meta WHERE key=?1", [&pending])?;
    }
    Ok(())
}

fn cursor(c: &Connection) -> Result<Option<Cursor>> {
    let saved = meta_get(c, CURSOR)?
        .map(|raw| serde_json::from_str::<Cursor>(&raw))
        .transpose()
        .map_err(StoreError::from)?;
    if saved.as_ref().is_some_and(|saved| saved.number < 0) {
        return Err(invalid());
    }
    Ok(saved)
}

fn save_cursor(c: &Connection, epoch: String, number: i64) -> Result<()> {
    meta_set(
        c,
        CURSOR,
        &serde_json::to_string(&Cursor { epoch, number }).map_err(StoreError::from)?,
    )?;
    Ok(())
}

/// Run before the server accepts new requests, even with similarity disabled.
/// A fresh database without a cursor retains the first-enable scan behavior.
pub fn reconcile_cursor(tx: &mut Tx<'_>) -> Result<()> {
    let c = tx.conn();
    let epoch = judgment_state::epoch(c)?;
    if cursor(c)?.is_some_and(|saved| saved.epoch != epoch) {
        let last = c.query_row(
            "SELECT COALESCE(MAX(number),0) FROM object_refs",
            [],
            |row| row.get(0),
        )?;
        save_cursor(c, epoch, last)?;
    }
    Ok(())
}

/// Scan only a small advancing page of globally issued numbers. Per new source,
/// inspect a bounded previous window from its own crew and choose at most one
/// pair from the same public channel. This is deliberately a partial scan.
pub fn observe_page(tx: &mut Tx<'_>) -> Result<()> {
    let c = tx.conn();
    let Some(config) = config(c)? else {
        return Ok(());
    };
    let epoch = judgment_state::epoch(c)?;
    let cursor = cursor(c)?;
    if cursor.as_ref().is_some_and(|saved| saved.epoch != epoch) {
        // A restored database may no longer contain attempts after its
        // snapshot. Do not enroll old anchors as fresh work on the new epoch.
        return reconcile_cursor(tx);
    }
    let after = cursor.map_or(0, |saved| saved.number);
    let anchors = {
        let mut q =
            c.prepare("SELECT number FROM object_refs WHERE number>?1 ORDER BY number LIMIT ?2")?;
        q.query_map(params![after, ANCHOR_PAGE], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let Some(&last) = anchors.last() else {
        return Ok(());
    };
    let authority = Authority::load(c, config)?;
    for second in anchors {
        let Some(b) = authority.source(tx.conn(), second)? else {
            continue;
        };
        let candidates = {
            let mut q = tx.conn().prepare(
                "SELECT r.number FROM object_refs r JOIN todos t ON t.id=r.raw_todo_id
                 WHERE r.number>=?1 AND r.number<?2 AND r.crew_id=?3 AND r.kind='requirement'
                   AND t.deleted_at IS NULL AND CASE WHEN json_valid(t.extra)
                       THEN json_extract(t.extra,'$.sourceChannelId')=?4 ELSE 0 END
                 ORDER BY r.number DESC LIMIT ?5",
            )?;
            q.query_map(
                params![
                    second.saturating_sub(NEARBY_IDS),
                    second,
                    b.crew,
                    b.channel,
                    CANDIDATE_LIMIT
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for first in candidates {
            let Some(a) = authority.source(tx.conn(), first)? else {
                continue;
            };
            if a.channel != b.channel {
                continue;
            }
            let id = pair_id(&b.crew, first, second)?;
            if !quarantined(tx.conn(), &id)?
                && load(tx.conn(), &id)?.is_none()
                && let Some(snapshot) = authority.pair(tx.conn(), first, a, second, b)?
            {
                save(
                    tx,
                    &id,
                    &Record {
                        version: 1,
                        crew: snapshot.crew,
                        first,
                        second,
                        fingerprint: snapshot.fingerprint,
                        state: State::Prepared,
                    },
                )?;
            }
            break;
        }
    }
    save_cursor(tx.conn(), epoch, last)
}

fn indexed_page(
    c: &Connection,
    prefix: &str,
    after: Option<&str>,
    limit: u16,
) -> Result<KeysetPage<String>> {
    let mut q =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let lower = after.map_or_else(|| prefix.into(), |id| format!("{prefix}{id}"));
    let mut rows = q
        .query_map(
            params![lower, format!("{prefix}\u{10ffff}"), i64::from(limit) + 1],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .map(|r| {
            let (key, raw) = r?;
            let id = key.strip_prefix(prefix).ok_or_else(invalid)?.to_string();
            Ok((id, raw))
        })
        .collect::<Result<Vec<_>>>()?;
    let next = if rows.len() > usize::from(limit) {
        rows.pop();
        rows.last().map(|(id, _)| id.clone())
    } else {
        None
    };
    Ok((rows, next))
}

fn quarantined(c: &Connection, id: &str) -> Result<bool> {
    let prefix = format!("{QUARANTINE}{id}:");
    Ok(c.query_row(
        "SELECT 1 FROM meta WHERE key>?1 AND key<?2 LIMIT 1",
        params![prefix, format!("{prefix}\u{10ffff}")],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

fn quarantine(tx: &mut Tx<'_>, id: &str, witness: &str) -> Result<()> {
    // Move only the corrupt index marker. Keep the primary row byte-for-byte;
    // even when it is missing, this witness fences the pair from rediscovery.
    let key = format!("{QUARANTINE}{id}:{}", judgments::fingerprint(&witness)?);
    tx.conn().execute(
        "INSERT OR IGNORE INTO meta(key,value) VALUES(?1,?2)",
        params![key, witness],
    )?;
    if meta_get(tx.conn(), &key)?.as_deref() != Some(witness) {
        return Err(invalid());
    }
    tx.conn()
        .execute("DELETE FROM meta WHERE key=?1", [format!("{PENDING}{id}")])?;
    Ok(())
}

fn pending_records(tx: &mut Tx<'_>, after: Option<&str>) -> Result<KeysetPage<Record>> {
    let (rows, next) = indexed_page(tx.conn(), PENDING, after, PENDING_PAGE)?;
    let mut records = Vec::new();
    for (id, witness) in rows {
        if witness == id
            && !quarantined(tx.conn(), &id)?
            && let Some(raw) = meta_get(tx.conn(), &format!("{RECORD}{id}"))?
            && let Ok(record) = decode(&id, &raw)
            && matches!(record.state, State::Prepared | State::Dispatched)
        {
            records.push((id, record));
        } else {
            quarantine(tx, &id, &witness)?;
        }
    }
    Ok((records, next))
}

pub fn pending_page(
    tx: &mut Tx<'_>,
    after: Option<String>,
) -> Result<(Vec<String>, Option<String>)> {
    let (records, next) = pending_records(tx, after.as_deref())?;
    let ids = records
        .into_iter()
        .filter_map(|(id, r)| matches!(r.state, State::Prepared).then_some(id))
        .collect();
    Ok((ids, next))
}

pub fn recover_page(tx: &mut Tx<'_>, after: Option<String>) -> Result<Option<String>> {
    let (records, next) = pending_records(tx, after.as_deref())?;
    for (id, mut record) in records {
        if matches!(record.state, State::Dispatched) {
            record.state = State::Unknown;
            save(tx, &id, &record)?;
        }
    }
    Ok(next)
}

fn current(c: &Connection, record: &Record) -> Result<Option<Snapshot>> {
    let Some(config) = config(c)? else {
        return Ok(None);
    };
    Ok(Authority::load(c, config)?
        .snapshot(c, record)?
        .filter(|snapshot| snapshot.fingerprint == record.fingerprint))
}

fn active_pending(c: &Connection, id: &str) -> Result<bool> {
    Ok(meta_get(c, &format!("{PENDING}{id}"))?.as_deref() == Some(id) && !quarantined(c, id)?)
}

pub fn dispatch(tx: &mut Tx<'_>, id: &str) -> Result<Option<Snapshot>> {
    if !active_pending(tx.conn(), id)? {
        return Ok(None);
    }
    let Some(mut record) = load(tx.conn(), id)? else {
        return Ok(None);
    };
    if !matches!(record.state, State::Prepared) {
        return Ok(None);
    }
    let snapshot = current(tx.conn(), &record)?;
    record.state = if snapshot.is_some() {
        State::Dispatched
    } else {
        State::Stale
    };
    save(tx, id, &record)?;
    Ok(snapshot)
}

pub fn still_current(c: &Connection, snapshot: &Snapshot) -> Result<bool> {
    if !active_pending(c, &snapshot.id)? {
        return Ok(false);
    }
    let Some(record) = load(c, &snapshot.id)? else {
        return Ok(false);
    };
    Ok(matches!(record.state, State::Dispatched)
        && snapshot.fingerprint == record.fingerprint
        && current(c, &record)?.is_some())
}

pub fn settle(tx: &mut Tx<'_>, snapshot: &Snapshot, outcome: Outcome) -> Result<()> {
    if !active_pending(tx.conn(), &snapshot.id)? {
        return Ok(());
    }
    let Some(mut record) = load(tx.conn(), &snapshot.id)? else {
        return Ok(());
    };
    if !matches!(record.state, State::Dispatched) || snapshot.fingerprint != record.fingerprint {
        return Ok(());
    }
    record.state = if current(tx.conn(), &record)?.is_none() {
        State::Stale
    } else {
        match outcome {
            Outcome::Answered {
                probability,
                suggested,
            } if (0.0..=1.0).contains(&probability) => State::Answered {
                probability,
                suggested,
                at: tx.now_ms(),
            },
            _ => State::Unavailable,
        }
    };
    save(tx, &snapshot.id, &record)
}

pub fn list(
    c: &Connection,
    crew: &str,
    after: Option<String>,
    limit: u16,
) -> Result<SimilarRequirementsPage> {
    crews::get(c, crew)?;
    if !(1..=100).contains(&limit) {
        return Err(invalid());
    }
    let config = config(c)?;
    let enabled = config.is_some();
    let mut page = SimilarRequirementsPage {
        crew_id: crew.into(),
        enabled,
        items: Vec::new(),
        next_after: None,
    };
    let Some(config) = config else {
        return Ok(page);
    };
    if crews::is_archived(c, crew)? {
        return Ok(page);
    }
    let hash = judgments::fingerprint(&crew)?;
    let prefix = format!("{RECORD}{hash}:");
    let scope = judgments::fingerprint(&(judgment_state::epoch(c)?, &hash))?;
    let after = after
        .as_deref()
        .map(|cursor| {
            let (witness, pair) = cursor.split_once(':').ok_or_else(invalid)?;
            let (a, b) = pair.split_once(':').ok_or_else(invalid)?;
            let first = a.parse::<i64>().map_err(|_| invalid())?;
            let second = b.parse::<i64>().map_err(|_| invalid())?;
            if witness != scope
                || first <= 0
                || first >= second
                || format!("{first:020}:{second:020}") != pair
            {
                return Err(invalid());
            }
            Ok(pair)
        })
        .transpose()?;
    let (rows, next) = indexed_page(c, &prefix, after, limit)?;
    let authority = Authority::load(c, config)?;
    for (pair, raw) in rows {
        let id = format!("{hash}:{pair}");
        if quarantined(c, &id)? {
            continue;
        }
        let record = decode(&id, &raw)?;
        if let State::Answered {
            probability,
            suggested: true,
            at,
        } = record.state
            && let Some(snapshot) = authority.snapshot(c, &record)?
            && snapshot.fingerprint == record.fingerprint
        {
            let (review_revision, effective) = crate::requirement_review::candidate_state(
                c,
                crew,
                record.first,
                record.second,
                &snapshot.review_witness,
            )?;
            if effective {
                continue;
            }
            let candidate_revision = snapshot.candidate_revision()?;
            page.items.push(SimilarRequirement {
                first_id: snapshot.first_id,
                second_id: snapshot.second_id,
                channel_id: snapshot.channel,
                candidate_revision,
                review_revision,
                probability,
                observed_at_ms: at,
            });
        }
    }
    page.next_after = next.map(|pair| format!("{scope}:{pair}"));
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        requirements,
        testkit::{ok, store},
    };
    use crewd_proto::{
        TodoCloseParams, TodoCreateParams,
        judgments::{JudgmentConfig, JudgmentSettingsSet},
    };
    use crewd_store::Store;

    fn configure(s: &mut Store, similar_requirements: Option<bool>) {
        let expected_revision = judgments::configuration(s.conn()).unwrap().revision;
        ok(s, |tx| {
            judgments::set_configuration(
                tx,
                &Sender::Human,
                JudgmentSettingsSet {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    credential_update: Default::default(),
                    expected_revision,
                    config: Some(JudgmentConfig {
                        enabled: true,
                        provider: "typesafe".into(),
                        base_url: "https://example.invalid".into(),
                        model: "fixture".into(),
                        choice_min: 0.5,
                        wake_yes: 0.5,
                        credential_source: "file".into(),
                        similar_requirements,
                    }),
                },
            )
        });
    }

    fn requirement(s: &mut Store, crew: &str, channel: &str, text: &str) -> String {
        ok(s, |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: crew.into(),
                    channel_id: Some(channel.into()),
                    text: text.into(),
                },
            )
        })
        .todo
        .requirement_id
        .unwrap()
    }

    fn prepare_pair(s: &mut Store) -> (String, String, String) {
        let first = requirement(s, "crew-a", "crew-a/lobby", "整理上线操作手册");
        let second = requirement(s, "crew-a", "crew-a/lobby", "编写发布步骤指南");
        ok(s, observe_page);
        let (ids, _) = ok(s, |tx| pending_page(tx, None));
        assert_eq!(ids.len(), 1);
        (first, second, ids[0].clone())
    }

    #[test]
    fn requirement_review_is_exact_reversible_and_durable_without_mutating_originals() {
        use crate::requirement_review as review;
        use crewd_proto::{
            RequirementReviewDisposition as D, RequirementReviewParams, RequirementReviewsParams,
        };
        let mut s = store();
        configure(&mut s, Some(true));
        let (first, second, id) = prepare_pair(&mut s);
        let snapshot = ok(&mut s, |tx| dispatch(tx, &id)).unwrap();
        ok(&mut s, |tx| {
            settle(
                tx,
                &snapshot,
                Outcome::Answered {
                    probability: 0.92,
                    suggested: true,
                },
            )
        });
        let p = RequirementReviewsParams {
            crew_id: "crew-a".into(),
            after: None,
            limit: None,
        };
        let candidate = list(s.conn(), "crew-a", None, 10).unwrap().items.remove(0);
        let mut action = RequirementReviewParams {
            crew_id: p.crew_id.clone(),
            first_id: first.clone(),
            second_id: second.clone(),
            expected_review_revision: candidate.review_revision,
            disposition: D::Group,
            candidate_revision: Some(candidate.candidate_revision),
            note: " preserve original scope ".into(),
        };
        let original_a = requirements::get(s.conn(), &first).unwrap();
        let original_b = requirements::get(s.conn(), &second).unwrap();
        let mut foreign = action.clone();
        foreign.crew_id = "crew-b".into();
        assert!(crate::run(&mut s, crate::testkit::NOW, |tx| review::apply(tx, foreign)).is_err());
        let mut stale = action.clone();
        stale.candidate_revision = Some("0".repeat(64));
        assert!(crate::run(&mut s, crate::testkit::NOW, |tx| review::apply(tx, stale)).is_err());
        let written = ok(&mut s, |tx| review::apply(tx, action.clone()));
        assert_eq!(written.revision, "1");
        assert_eq!(written.note, action.note);
        assert_eq!(written.channel_id, candidate.channel_id);
        assert!(list(s.conn(), "crew-a", None, 10).unwrap().items.is_empty());
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| review::apply(
                tx,
                action.clone()
            ))
            .is_err()
        );
        assert_eq!(requirements::get(s.conn(), &first).unwrap(), original_a);
        assert_eq!(requirements::get(s.conn(), &second).unwrap(), original_b);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let mut s = Store::open(&path).unwrap();
        let changes = s.conn().total_changes();
        let row = review::list(s.conn(), &p).unwrap().items.remove(0);
        assert_eq!(row.review, written);
        assert!(row.effective);
        assert_eq!(s.conn().total_changes(), changes);

        let clear = RequirementReviewParams {
            candidate_revision: None,
            expected_review_revision: "1".into(),
            disposition: D::Clear,
            note: "review again".into(),
            ..action.clone()
        };
        let cleared = ok(&mut s, |tx| review::apply(tx, clear));
        assert_eq!(cleared.revision, "2");
        assert!(cleared.candidate_revision.is_none());
        let visible_again = list(s.conn(), "crew-a", None, 10).unwrap().items.remove(0);
        assert_eq!(visible_again.review_revision, "2");
        action.expected_review_revision = "2".into();
        action.disposition = D::Dismiss;
        let dismissed = ok(&mut s, |tx| review::apply(tx, action.clone()));
        configure(&mut s, None);
        assert!(!list(s.conn(), "crew-a", None, 10).unwrap().enabled);
        assert!(review::list(s.conn(), &p).unwrap().items[0].effective);
        ok(&mut s, |tx| {
            requirements::close(
                tx,
                &Sender::Human,
                TodoCloseParams {
                    id: first.clone(),
                    expected_status: "unclaimed".into(),
                    note: "original separately closed".into(),
                },
            )
        });
        assert!(review::list(s.conn(), &p).unwrap().items[0].effective);
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        let historical = review::list(s.conn(), &p).unwrap().items.remove(0);
        assert_eq!(historical.review, dismissed);
        assert!(!historical.effective);
        let clear = RequirementReviewParams {
            candidate_revision: None,
            expected_review_revision: "3".into(),
            disposition: D::Clear,
            note: "clear historical choice".into(),
            ..action
        };
        assert_eq!(ok(&mut s, |tx| review::apply(tx, clear)).revision, "4");
        assert_eq!(requirements::get(s.conn(), &second).unwrap(), original_b);
    }

    #[test]
    fn requirement_review_archive_restore_fences_the_human_form_without_rescoring() {
        use crate::{channels, requirement_review as review};
        use crewd_proto::{
            RequirementReviewDisposition as D, RequirementReviewParams, RequirementReviewsParams,
        };
        let mut s = store();
        configure(&mut s, Some(true));
        let channel = ok(&mut s, |tx| {
            channels::create(
                tx,
                &Sender::Human,
                channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "fixture topic".into(),
                    purpose: None,
                    members: vec![],
                },
            )
        });
        let first = requirement(&mut s, "crew-a", &channel.id, "original one");
        let second = requirement(&mut s, "crew-a", &channel.id, "original two");
        ok(&mut s, observe_page);
        let (pending, _) = ok(&mut s, |tx| pending_page(tx, None));
        let snapshot = ok(&mut s, |tx| dispatch(tx, &pending[0])).unwrap();
        ok(&mut s, |tx| {
            settle(
                tx,
                &snapshot,
                Outcome::Answered {
                    probability: 0.91,
                    suggested: true,
                },
            )
        });
        let old = list(s.conn(), "crew-a", None, 10).unwrap().items.remove(0);
        let mut command = RequirementReviewParams {
            crew_id: "crew-a".into(),
            first_id: first,
            second_id: second,
            expected_review_revision: old.review_revision,
            disposition: D::Group,
            candidate_revision: Some(old.candidate_revision.clone()),
            note: "confirmed exact originals".into(),
        };
        ok(&mut s, |tx| review::apply(tx, command.clone()));
        let history = RequirementReviewsParams {
            crew_id: "crew-a".into(),
            after: None,
            limit: None,
        };
        for archived in [true, false] {
            ok(&mut s, |tx| {
                channels::update(
                    tx,
                    &Sender::Human,
                    &channel.id,
                    channels::ChannelPatch {
                        archived: Some(archived),
                        ..Default::default()
                    },
                )
            });
            assert!(!review::list(s.conn(), &history).unwrap().items[0].effective);
        }
        let current = list(s.conn(), "crew-a", None, 10).unwrap().items.remove(0);
        assert_eq!(current.observed_at_ms, old.observed_at_ms);
        assert_eq!(current.probability, old.probability);
        assert_ne!(current.candidate_revision, old.candidate_revision);
        assert_eq!(current.review_revision, "1");
        command.expected_review_revision = current.review_revision;
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| review::apply(
                tx,
                command.clone()
            ))
            .is_err()
        );
        command.candidate_revision = Some(current.candidate_revision);
        assert_eq!(ok(&mut s, |tx| review::apply(tx, command)).revision, "2");
        assert!(review::list(s.conn(), &history).unwrap().items[0].effective);
    }

    #[test]
    fn old_opt_in_does_not_scan_then_real_same_channel_sources_remain_separate() {
        let mut s = store();
        configure(&mut s, None);
        let first = requirement(&mut s, "crew-a", "crew-a/lobby", "整理上线操作手册");
        requirement(&mut s, "crew-b", "crew-b/lobby", "不应发送的其他组内容");
        let second = requirement(&mut s, "crew-a", "crew-a/lobby", "编写发布步骤指南");
        ok(&mut s, observe_page);
        assert!(meta_get(s.conn(), CURSOR).unwrap().is_none());
        assert!(!list(s.conn(), "crew-a", None, 10).unwrap().enabled);
        configure(&mut s, Some(true));
        ok(&mut s, observe_page);
        let (ids, _) = ok(&mut s, |tx| pending_page(tx, None));
        assert_eq!(ids.len(), 1);
        let snapshot = ok(&mut s, |tx| dispatch(tx, &ids[0])).unwrap();
        assert_eq!((&snapshot.first_id, &snapshot.second_id), (&first, &second));
        assert!(still_current(s.conn(), &snapshot).unwrap());
        assert!(ok(&mut s, |tx| dispatch(tx, &ids[0])).is_none());
        ok(&mut s, |tx| {
            settle(
                tx,
                &snapshot,
                Outcome::Answered {
                    probability: 0.92,
                    suggested: true,
                },
            )
        });
        let page = list(s.conn(), "crew-a", None, 10).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].first_id, first);
        let public = serde_json::to_string(&page).unwrap();
        assert!(!public.contains("手册") && !public.contains("指南"));
        assert!(list(s.conn(), "crew-b", None, 10).unwrap().items.is_empty());
        for id in [first, second] {
            assert_eq!(
                requirements::get(s.conn(), &id).unwrap().todo.status,
                "unclaimed"
            );
        }
    }

    #[test]
    fn dispatched_recovery_never_reissues_a_pair_or_adopts_a_late_answer() {
        let mut s = store();
        configure(&mut s, Some(true));
        let (_, _, id) = prepare_pair(&mut s);
        let snapshot = ok(&mut s, |tx| dispatch(tx, &id)).unwrap();
        assert!(ok(&mut s, |tx| recover_page(tx, None)).is_none());
        ok(&mut s, observe_page);
        assert!(ok(&mut s, |tx| pending_page(tx, None)).0.is_empty());
        assert!(ok(&mut s, |tx| dispatch(tx, &id)).is_none());
        assert!(!still_current(s.conn(), &snapshot).unwrap());
        ok(&mut s, |tx| {
            settle(
                tx,
                &snapshot,
                Outcome::Answered {
                    probability: 1.0,
                    suggested: true,
                },
            )
        });
        assert!(matches!(
            load(s.conn(), &id).unwrap().unwrap().state,
            State::Unknown
        ));
        assert!(list(s.conn(), "crew-a", None, 10).unwrap().items.is_empty());
    }

    #[test]
    fn source_closure_and_stop_invalidate_dispatched_pairs() {
        let mut s = store();
        configure(&mut s, Some(true));
        let (first, _, id) = prepare_pair(&mut s);
        let snapshot = ok(&mut s, |tx| dispatch(tx, &id)).unwrap();
        ok(&mut s, |tx| {
            requirements::close(
                tx,
                &Sender::Human,
                TodoCloseParams {
                    id: first,
                    expected_status: "unclaimed".into(),
                    note: "已裁定".into(),
                },
            )
        });
        assert!(!still_current(s.conn(), &snapshot).unwrap());
        ok(&mut s, |tx| {
            settle(
                tx,
                &snapshot,
                Outcome::Answered {
                    probability: 1.0,
                    suggested: true,
                },
            )
        });
        assert!(matches!(
            load(s.conn(), &id).unwrap().unwrap().state,
            State::Stale
        ));
        let mut stopped = store();
        configure(&mut stopped, Some(true));
        let (_, _, second_id) = prepare_pair(&mut stopped);
        let snapshot = ok(&mut stopped, |tx| dispatch(tx, &second_id)).unwrap();
        ok(&mut stopped, |tx| wakes::human_stop(tx, "cap-a"));
        assert!(!still_current(stopped.conn(), &snapshot).unwrap());
    }

    #[test]
    fn startup_restore_baseline_does_not_consume_a_later_anchor() {
        let mut s = store();
        configure(&mut s, Some(true));
        let (_, previous, old_pair) = prepare_pair(&mut s);
        ok(&mut s, |tx| {
            judgment_state::restored(tx.conn())?;
            Ok(())
        });
        configure(&mut s, None);
        ok(&mut s, reconcile_cursor);
        let new = requirement(&mut s, "crew-a", "crew-a/lobby", "恢复后的新需求");
        configure(&mut s, Some(true));
        ok(&mut s, observe_page);
        let (ids, _) = ok(&mut s, |tx| pending_page(tx, None));
        assert!(ids.contains(&old_pair));
        assert!(ok(&mut s, |tx| dispatch(tx, &old_pair)).is_none());
        let new_pair = ids.iter().find(|id| **id != old_pair).unwrap();
        let snapshot = ok(&mut s, |tx| dispatch(tx, new_pair)).unwrap();
        assert_eq!((snapshot.first_id, snapshot.second_id), (previous, new));
    }

    #[test]
    fn corrupt_pending_quarantines_exact_marker_without_blocking_healthy_pair() {
        let mut s = store();
        configure(&mut s, Some(true));
        let (_, previous, damaged) = prepare_pair(&mut s);
        let new = requirement(&mut s, "crew-a", "crew-a/lobby", "相同群的新手册需求");
        ok(&mut s, observe_page);
        let malformed = "{invalid primary JSON";
        let marker = "original corrupt index witness";
        meta_set(s.conn(), &format!("{RECORD}{damaged}"), malformed).unwrap();
        meta_set(s.conn(), &format!("{PENDING}{damaged}"), marker).unwrap();
        assert!(ok(&mut s, |tx| recover_page(tx, None)).is_none());
        assert_eq!(
            meta_get(s.conn(), &format!("{RECORD}{damaged}"))
                .unwrap()
                .as_deref(),
            Some(malformed)
        );
        assert!(
            meta_get(s.conn(), &format!("{PENDING}{damaged}"))
                .unwrap()
                .is_none()
        );
        let quarantine_key = format!(
            "{QUARANTINE}{damaged}:{}",
            judgments::fingerprint(&marker).unwrap()
        );
        assert_eq!(
            meta_get(s.conn(), &quarantine_key).unwrap().as_deref(),
            Some(marker)
        );
        let (ids, _) = ok(&mut s, |tx| pending_page(tx, None));
        assert_eq!(ids.len(), 1);
        let snapshot = ok(&mut s, |tx| dispatch(tx, &ids[0])).unwrap();
        assert_eq!(
            (snapshot.first_id.as_str(), snapshot.second_id.as_str()),
            (previous.as_str(), new.as_str())
        );
        ok(&mut s, |tx| {
            settle(
                tx,
                &snapshot,
                Outcome::Answered {
                    probability: 0.91,
                    suggested: true,
                },
            )
        });
        assert_eq!(list(s.conn(), "crew-a", None, 20).unwrap().items.len(), 1);
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key IN (?1,?2)",
                params![CURSOR, format!("{RECORD}{damaged}")],
            )
            .unwrap();
        ok(&mut s, observe_page);
        assert!(ok(&mut s, |tx| pending_page(tx, None)).0.is_empty());
        assert!(ok(&mut s, |tx| dispatch(tx, &damaged)).is_none());
    }
}
