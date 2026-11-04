//! Same-crew Topic responsibility episodes from complete, authoritative linked Work.
use crate::{CollabError, Result, Tx, asks, channels, crews, judgments, wakes, work};
use crewd_proto::{
    ChannelKind, ChannelResponsibility as View, ChannelResponsibilityHandled as Handled,
    ChannelResponsibilityNoticePhase as NoticePhase, ChannelResponsibilityPage,
    ChannelResponsibilityParams, ChannelResponsibilityPhase as Phase, Desired,
};
use crewd_store::{StoreError, judgment_state, meta_get, meta_set, object_refs::ObjectKind};
use crewd_work::{Event, EventKind, WorkBook, WorkId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SOURCE: &str = "patrol-channel-responsibility:";
const PAGE: usize = 32;
const WORK_CAP: usize = 32;
const EVENTS: usize = 64;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceWork {
    id: String,
    core: WorkId,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Topology {
    generation: Option<String>,
    link_revision: i64,
    links: Vec<SourceWork>,
}
struct Facts<'a> {
    topology: Topology,
    live: Vec<SourceWork>,
    book: &'a WorkBook,
}
impl Facts<'_> {
    fn unowned(&self) -> bool {
        !self.live.is_empty()
            && self.live.iter().all(|w| {
                self.book
                    .item(w.core)
                    .is_some_and(|item| item.owner.is_none())
            })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    crew: String,
    channel: String,
    episode: String,
    captain: String,
    generation: String,
    stop_epoch: String,
    epoch: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notice {
    target: Target,
    wake: i64,
    claimed: bool,
    delivered: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Episode {
    crew: String,
    channel: String,
    id: String,
    epoch: String,
    phase: Phase,
    topology: Topology,
    original: Vec<SourceWork>,
    observed_at: i64,
    updated_at: i64,
    observed_seq: u64,
    event_cursor: u64,
    clear_epoch: Option<String>,
    handled: Option<Handled>,
    notice: Option<Notice>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    episode: Episode,
    seal: String,
}

fn invalid() -> CollabError {
    StoreError::Invalid("invalid channel responsibility evidence".into()).into()
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn valid_channel(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn prefix(crew: &str) -> String {
    format!("channel.responsibility.v1:{}:{crew}:", crew.len())
}
fn key(crew: &str, channel: &str) -> String {
    format!("{}{channel}", prefix(crew))
}
fn save(c: &Connection, episode: Episode) -> Result<()> {
    let seal = judgments::fingerprint(&episode)?;
    let key = key(&episode.crew, &episode.channel);
    meta_set(
        c,
        &key,
        &serde_json::to_string(&Saved { episode, seal }).map_err(StoreError::from)?,
    )?;
    Ok(())
}
// Reverse point watches let an assignment settle affected episodes immediately
// without scanning unrelated topics or traversing the crew's entire link table.
fn watch_prefix(crew: &str, id: WorkId) -> String {
    format!(
        "channel.responsibility.watch.v1:{}:{crew}:{id}:",
        crew.len()
    )
}
fn save_open(c: &Connection, e: Episode) -> Result<()> {
    for work in &e.original {
        meta_set(
            c,
            &format!("{}{}", watch_prefix(&e.crew, work.core), e.channel),
            &e.id,
        )?;
    }
    save(c, e)
}
fn clear_watches(c: &Connection, e: &Episode) -> Result<()> {
    for work in &e.original {
        c.execute(
            "DELETE FROM meta WHERE key=?1 AND value=?2",
            (
                format!("{}{}", watch_prefix(&e.crew, work.core), e.channel),
                &e.id,
            ),
        )?;
    }
    Ok(())
}
fn decode(raw: &str, crew: &str, channel: &str) -> Result<Episode> {
    let Saved { episode, seal } = serde_json::from_str(raw).map_err(StoreError::from)?;
    if episode.crew != crew
        || episode.channel != channel
        || !valid_channel(channel)
        || !uuid(&episode.id)
        || episode.observed_at > episode.updated_at
        || episode.event_cursor < episode.observed_seq
        || episode.original.is_empty()
        || episode.original.len() > WORK_CAP
        || (episode.phase == Phase::Handled) != episode.handled.is_some()
        || judgments::fingerprint(&episode)? != seal
        || episode.notice.as_ref().is_some_and(|n| {
            n.wake <= 0
                || (n.delivered && !n.claimed)
                || n.target.crew != crew
                || n.target.channel != channel
                || n.target.episode != episode.id
                || n.target.epoch != episode.epoch
        })
    {
        return Err(invalid());
    }
    Ok(episode)
}
fn load(c: &Connection, crew: &str, channel: &str) -> Result<Option<Episode>> {
    meta_get(c, &key(crew, channel))?
        .map(|raw| decode(&raw, crew, channel))
        .transpose()
}
type RawPage = (Vec<(String, String)>, Option<String>);
fn page_rows(c: &Connection, crew: &str, after: Option<&str>) -> Result<RawPage> {
    let prefix = prefix(crew);
    let lower = format!("{prefix}{}", after.unwrap_or(""));
    let upper = format!("{};", prefix.strip_suffix(':').expect("key suffix"));
    let mut q =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let mut rows = q
        .query_map(params![lower, upper, (PAGE + 1) as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > PAGE;
    rows.truncate(PAGE);
    let rows = rows
        .into_iter()
        .map(|(key, raw)| {
            let channel = key.strip_prefix(&prefix).ok_or_else(invalid)?.to_owned();
            Ok((channel, raw))
        })
        .collect::<Result<Vec<_>>>()?;
    let next = more.then(|| rows.last().expect("full page").0.clone());
    Ok((rows, next))
}

fn page(c: &Connection, crew: &str, after: Option<&str>) -> Result<(Vec<Episode>, Option<String>)> {
    let (raw, next) = page_rows(c, crew, after)?;
    let episodes = raw
        .into_iter()
        .map(|(channel, raw)| decode(&raw, crew, &channel))
        .collect::<Result<Vec<_>>>()?;
    Ok((episodes, next))
}

fn facts<'a>(
    c: &Connection,
    book: &'a WorkBook,
    crew: &str,
    channel: &str,
) -> Result<Option<Facts<'a>>> {
    let ch = match channels::get(c, channel) {
        Ok(ch) => ch,
        Err(CollabError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if ch.crew_id != crew
        || ch.kind != ChannelKind::Topic
        || ch.archived_at.is_some()
        || crews::is_archived(c, crew)?
        || crews::get(c, crew)?.removed_at.is_some()
    {
        return Ok(None);
    }
    let mut items = BTreeMap::new();
    for (&core, item) in &book.state().items {
        if item
            .channel
            .as_ref()
            .is_none_or(|ch| ch.as_str() != channel)
        {
            continue;
        }
        if items.len() == WORK_CAP
            || book
                .state()
                .item_crew(core)
                .is_none_or(|c| c.as_str() != crew)
        {
            return Ok(None);
        }
        let Some(r) = crewd_store::work_organization::reference_for_core(
            c,
            ObjectKind::Plan,
            &core.to_string(),
        )?
        .filter(|r| r.crew_id == crew && r.number > 0) else {
            return Ok(None);
        };
        items.insert(r.number, SourceWork { id: r.id(), core });
    }
    let mut q = c.prepare(
        "SELECT object_kind,object_id,source_crew_id FROM channel_work_links
        WHERE channel_id=?1 ORDER BY object_kind,object_id LIMIT ?2",
    )?;
    let links = q
        .query_map(params![channel, (WORK_CAP + 1) as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if links.len() > WORK_CAP {
        return Ok(None);
    }
    for (kind, id, source) in links {
        if kind != "work" {
            if !verified_nonwork(c, book, &kind, &id, &source)? {
                return Ok(None);
            }
            continue;
        }
        if source != crew {
            return Ok(None);
        }
        let Ok(number) = work::number(&id) else {
            return Ok(None);
        };
        let Some(reference) = crewd_store::object_refs::get(c, ObjectKind::Plan, number)?
            .filter(|r| r.crew_id == crew)
        else {
            return Ok(None);
        };
        let core = work::core_plan(c, &reference)?;
        if book
            .state()
            .item_crew(core)
            .is_none_or(|c| c.as_str() != crew)
            || book.item(core).is_none()
        {
            return Ok(None);
        }
        items.insert(number, SourceWork { id, core });
        if items.len() > WORK_CAP {
            return Ok(None);
        }
    }
    let links: Vec<_> = items.into_values().collect();
    let live = links
        .iter()
        .filter(|w| book.item(w.core).is_some_and(|it| it.status.is_live()))
        .cloned()
        .collect();
    Ok(Some(Facts {
        topology: Topology {
            generation: crate::channel_archive::generation(c, channel)?,
            link_revision: crewd_store::channel_work::revision(c, channel)?,
            links,
        },
        live,
        book,
    }))
}

fn verified_nonwork(
    c: &Connection,
    book: &WorkBook,
    kind: &str,
    id: &str,
    crew: &str,
) -> Result<bool> {
    let kind = match kind {
        "requirement" => ObjectKind::Requirement,
        "humanTask" => ObjectKind::HumanTask,
        _ => return Ok(false),
    };
    let Some(number) = id
        .strip_prefix(kind.prefix())
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| *n > 0)
    else {
        return Ok(false);
    };
    if crewd_store::object_refs::get(c, kind, number)?
        .is_none_or(|r| r.crew_id != crew || r.id() != id)
    {
        return Ok(false);
    }
    let Some(core) = crewd_store::work_organization::core_for_reference(c, kind, number)? else {
        return Ok(false);
    };
    Ok(match kind {
        ObjectKind::Requirement => core.parse::<crewd_work::EntryNo>().ok().is_some_and(|id| {
            book.entry(id).is_some()
                && book
                    .state()
                    .entry_crew(id)
                    .is_some_and(|c| c.as_str() == crew)
        }),
        ObjectKind::HumanTask => core.parse::<crewd_work::AskId>().ok().is_some_and(|id| {
            book.ask(id).is_some()
                && book
                    .state()
                    .ask_crew(id)
                    .is_some_and(|c| c.as_str() == crew)
        }),
        _ => unreachable!(),
    })
}

fn matches_handled(e: &Episode, handled: &Handled, event: &Event) -> bool {
    event.seq > e.observed_seq
        && event.seq.to_string() == handled.event_seq
        && event.at.0 == handled.at_ms
        && e.original.iter().any(|w| {
            w.id == handled.work_id
                && matches!(&event.kind,
                EventKind::OwnerChanged { id, from: None, to, .. }
                    if *id == w.core && to.as_str() == handled.owner_session_id)
        })
}
fn verify_handled(c: &Connection, e: &Episode, proof: &Handled) -> Result<()> {
    let seq: i64 = proof.event_seq.parse().map_err(|_| invalid())?;
    let body: String = c
        .query_row(
            "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
            (seq, &e.crew),
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::from)?
        .ok_or_else(invalid)?;
    let event: Event = serde_json::from_str(&body).map_err(StoreError::from)?;
    if !matches_handled(e, proof, &event) {
        return Err(invalid());
    }
    Ok(())
}
fn find_handled(c: &Connection, e: &mut Episode, f: &Facts<'_>) -> Result<Option<Handled>> {
    let mut q = c.prepare(
        "SELECT seq,body FROM organization_work_events
        WHERE crew_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
    )?;
    let from = i64::try_from(e.event_cursor).map_err(|_| invalid())?;
    let rows = q
        .query_map(params![e.crew, from, EVENTS as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let exhausted = rows.len() < EVENTS;
    for (seq, body) in rows {
        let seq = u64::try_from(seq).map_err(|_| invalid())?;
        let event: Event = serde_json::from_str(&body).map_err(StoreError::from)?;
        if event.seq != seq || seq <= e.event_cursor {
            return Err(invalid());
        }
        e.event_cursor = seq;
        if let Some(proof) = handled_from_event(c, e, f, &event)? {
            return Ok(Some(proof));
        }
    }
    if exhausted {
        e.phase = Phase::Stale;
    }
    Ok(None)
}

fn handled_from_event(
    c: &Connection,
    e: &Episode,
    f: &Facts<'_>,
    event: &Event,
) -> Result<Option<Handled>> {
    let EventKind::OwnerChanged {
        id,
        from: None,
        to,
        note,
        ..
    } = &event.kind
    else {
        return Ok(None);
    };
    let Some(source) = e.original.iter().find(|w| w.core == *id) else {
        return Ok(None);
    };
    let Some(item) = f.book.item(*id) else {
        return Ok(None);
    };
    if event.seq <= e.observed_seq
        || item.owner.as_ref() != Some(to)
        || !item.owner_history.last().is_some_and(|last| {
            last.from.is_none()
                && last.to == *to
                && last.at == event.at
                && last.by == event.by
                && last.note == *note
        })
        || !crewd_store::sessions::get(c, to.as_str())?.is_some_and(|s| s.crew_id == e.crew)
    {
        return Ok(None);
    }
    Ok(Some(Handled {
        work_id: source.id.clone(),
        owner_session_id: to.to_string(),
        event_seq: event.seq.to_string(),
        at_ms: event.at.0,
    }))
}

fn settle(e: &mut Episode, epoch: &str, proof: Handled) {
    e.phase = Phase::Handled;
    e.clear_epoch = Some(epoch.into());
    e.updated_at = e.updated_at.max(proof.at_ms);
    e.handled = Some(proof);
}

pub(crate) fn reconcile_assignments(tx: &Tx<'_>, book: &WorkBook, events: &[Event]) -> Result<()> {
    let c = tx.conn();
    for event in events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::OwnerChanged { from: None, .. }))
        .take(WORK_CAP)
    {
        // The Work mutation remains authoritative if its optional episode evidence
        // cannot be reconciled. Periodic patrol can inspect it again.
        c.execute_batch("SAVEPOINT responsibility_assignment")?;
        if reconcile_assignment(c, book, event, tx.now_ms()).is_err() {
            c.execute_batch(
                "ROLLBACK TO responsibility_assignment; RELEASE responsibility_assignment",
            )?;
        } else {
            c.execute_batch("RELEASE responsibility_assignment")?;
        }
    }
    Ok(())
}

fn reconcile_assignment(c: &Connection, book: &WorkBook, event: &Event, now: i64) -> Result<()> {
    let EventKind::OwnerChanged { id, from: None, .. } = &event.kind else {
        return Ok(());
    };
    let Some(crew) = book.state().item_crew(*id) else {
        return Ok(());
    };
    let crew = crew.as_str();
    let prefix = watch_prefix(crew, *id);
    let upper = format!("{};", prefix.strip_suffix(':').ok_or_else(invalid)?);
    let mut q =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let rows = q
        .query_map(params![prefix, upper, PAGE as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let epoch = judgment_state::epoch(c)?;
    for (key, episode) in rows {
        let Some(channel) = key.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(Some(mut e)) = load(c, crew, channel) else {
            continue;
        };
        if e.id != episode || e.epoch != epoch || e.phase != Phase::Open {
            continue;
        }
        let Ok(Some(f)) = facts(c, book, crew, channel) else {
            continue;
        };
        if f.topology != e.topology {
            continue;
        }
        if let Some(proof) = handled_from_event(c, &e, &f, event)? {
            settle(&mut e, &epoch, proof);
            e.updated_at = e.updated_at.max(now);
            e.event_cursor = e.event_cursor.max(event.seq);
            clear_watches(c, &e)?;
            save(c, e)?;
        }
    }
    Ok(())
}

fn open(crew: &str, channel: &str, now: i64, epoch: &str, f: Facts<'_>) -> Episode {
    let seq = f.book.state().last_seq;
    Episode {
        crew: crew.into(),
        channel: channel.into(),
        id: uuid::Uuid::new_v4().to_string(),
        epoch: epoch.into(),
        phase: Phase::Open,
        topology: f.topology,
        original: f.live,
        observed_at: now,
        updated_at: now,
        observed_seq: seq,
        event_cursor: seq,
        clear_epoch: None,
        handled: None,
        notice: None,
    }
}

pub(crate) fn observe(tx: &Tx<'_>, crew: &str, channels: &[String]) -> Result<()> {
    if channels.is_empty() {
        return Ok(());
    }
    let c = tx.conn();
    let book = asks::load_org(c)?;
    let epoch = judgment_state::epoch(c)?;
    for channel in channels {
        c.execute_batch("SAVEPOINT responsibility_channel")?;
        // One damaged optional episode cannot stop other channels in the patrol.
        // This branch writes metadata only and never appends Tx hub changes.
        if observe_channel(c, &book, &epoch, crew, channel, tx.now_ms()).is_err() {
            c.execute_batch("ROLLBACK TO responsibility_channel; RELEASE responsibility_channel")?;
        } else {
            c.execute_batch("RELEASE responsibility_channel")?;
        }
    }
    Ok(())
}

fn observe_channel(
    c: &Connection,
    book: &WorkBook,
    epoch: &str,
    crew: &str,
    channel: &str,
    now: i64,
) -> Result<()> {
    let f = facts(c, book, crew, channel)?;
    let Some(mut e) = load(c, crew, channel)? else {
        if let Some(f) = f.filter(Facts::unowned) {
            save_open(c, open(crew, channel, now, epoch, f))?;
        }
        return Ok(());
    };
    if e.phase != Phase::Open {
        if let Some(f) = f {
            if f.unowned() && e.clear_epoch.as_deref() == Some(epoch) {
                save_open(c, open(crew, channel, now, epoch, f))?;
            } else if !f.unowned() && e.clear_epoch.as_deref() != Some(epoch) {
                e.clear_epoch = Some(epoch.into());
                e.updated_at = now.max(e.updated_at);
                save(c, e)?;
            }
        }
        return Ok(());
    }
    let previous_cursor = e.event_cursor;
    match f {
        Some(f) if e.epoch == epoch && f.topology == e.topology => {
            let assigned = e
                .original
                .iter()
                .any(|w| f.book.item(w.core).is_some_and(|item| item.owner.is_some()));
            if assigned {
                if let Some(proof) = find_handled(c, &mut e, &f)? {
                    settle(&mut e, epoch, proof);
                }
            } else if f.live.is_empty() {
                e.phase = Phase::Ended;
                e.clear_epoch = Some(epoch.into());
            }
        }
        evidence => {
            e.phase = Phase::Stale;
            if evidence.is_some_and(|f| !f.unowned()) {
                e.clear_epoch = Some(epoch.into());
            }
        }
    }
    if e.phase != Phase::Open {
        clear_watches(c, &e)?;
    }
    if e.phase != Phase::Open || e.event_cursor != previous_cursor {
        e.updated_at = now.max(e.updated_at);
        save(c, e)?;
    }
    Ok(())
}

pub fn is_notice(source: &str) -> bool {
    source.starts_with(SOURCE)
}
pub fn matches_generation(source: &str, note: Option<&str>, generation: &str) -> bool {
    note.and_then(|raw| serde_json::from_str::<Target>(raw).ok())
        .is_some_and(|target| {
            source.strip_prefix(SOURCE) == Some(&target.episode) && target.generation == generation
        })
}
fn owns(n: &Notice, wake: &wakes::WakeRow) -> bool {
    n.wake == wake.id
        && n.target.captain == wake.session
        && wake.kind == wakes::WakeKind::System
        && wake.priority == wakes::WakePriority::Auto
        && wake.source.strip_prefix(SOURCE) == Some(&n.target.episode)
        && wake
            .note
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Target>(raw).ok())
            .as_ref()
            == Some(&n.target)
}
fn eligible_captain(c: &Connection, crew: &str, channel: &str, captain: &str) -> Result<bool> {
    Ok(crews::captain_of(c, crew)?.as_deref() == Some(captain)
        && crewd_store::sessions::get(c, captain)?.is_some_and(|s| {
            s.crew_id == crew && s.desired == Desired::Running && s.resume_failed_at.is_none()
        })
        && !wakes::automatic_work_paused(c, captain)?
        && channels::readable_by(c, channel, captain)?)
}
fn current(e: &Episode, f: &Facts<'_>, epoch: &str) -> bool {
    e.phase == Phase::Open
        && e.epoch == epoch
        && e.topology == f.topology
        && f.live.iter().all(|w| e.original.contains(w))
        && f.unowned()
}
fn readable_sources(c: &Connection, f: &Facts<'_>, captain: &str) -> Result<bool> {
    for work in &f.live {
        if let Some(channel) = f
            .book
            .item(work.core)
            .and_then(|item| item.channel.as_ref())
            && !channels::readable_by(c, channel.as_str(), captain)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn enqueue_next(tx: &mut Tx<'_>, crew: &str, captain: &str, generation: &str) -> Result<()> {
    if generation.is_empty() {
        return Ok(());
    }
    let cursor = format!("channel.responsibility.dispatch.v1:{}:{crew}", crew.len());
    let after = meta_get(tx.conn(), &cursor)?.filter(|id| !id.is_empty());
    let (rows, next) = page_rows(tx.conn(), crew, after.as_deref())?;
    let epoch = judgment_state::epoch(tx.conn())?;
    let mut book = None;
    for (channel, raw) in rows {
        let Ok(mut e) = decode(&raw, crew, &channel) else {
            continue;
        };
        if e.notice.is_some() || e.phase != Phase::Open || e.epoch != epoch {
            continue;
        }
        if book.is_none() {
            book = Some(asks::load_org(tx.conn())?);
        }
        let Some(f) = facts(tx.conn(), book.as_ref().expect("loaded"), crew, &e.channel)? else {
            continue;
        };
        if !current(&e, &f, &epoch)
            || !eligible_captain(tx.conn(), crew, &e.channel, captain)?
            || !readable_sources(tx.conn(), &f, captain)?
        {
            continue;
        }
        let target = Target {
            crew: crew.into(),
            channel: e.channel.clone(),
            episode: e.id.clone(),
            captain: captain.into(),
            generation: generation.into(),
            stop_epoch: judgments::stop_epoch(tx.conn(), captain)?,
            epoch,
        };
        let wake = wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: captain.into(),
                kind: wakes::WakeKind::System,
                source: format!("{SOURCE}{}", e.id),
                priority: wakes::WakePriority::Auto,
                not_before_ms: None,
                note: Some(serde_json::to_string(&target).map_err(StoreError::from)?),
                origin: None,
            },
        )?;
        let wakes::Enqueued::New(wake) = wake else {
            return Err(invalid());
        };
        e.notice = Some(Notice {
            target,
            wake,
            claimed: false,
            delivered: false,
        });
        e.updated_at = tx.now_ms().max(e.updated_at);
        meta_set(tx.conn(), &cursor, &e.channel)?;
        return save(tx.conn(), e);
    }
    let next = next.unwrap_or_default();
    if after.as_deref().unwrap_or("") != next {
        meta_set(tx.conn(), &cursor, &next)?;
    }
    Ok(())
}
fn record_for_wake(c: &Connection, wake: &wakes::WakeRow) -> Result<Option<Episode>> {
    let Some(target) = wake
        .note
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Target>(raw).ok())
    else {
        return Ok(None);
    };
    Ok(load(c, &target.crew, &target.channel)?
        .filter(|e| e.notice.as_ref().is_some_and(|n| owns(n, wake))))
}
pub(crate) fn render(c: &Connection, wake: &wakes::WakeRow) -> Result<work::FollowupNote> {
    let e = match record_for_wake(c, wake) {
        Ok(Some(e)) => e,
        Ok(None) | Err(CollabError::Store(StoreError::Invalid(_) | StoreError::Json(_))) => {
            return Ok(work::FollowupNote::Stale);
        }
        Err(error) => return Err(error),
    };
    let notice = e.notice.as_ref().ok_or_else(invalid)?;
    if notice.claimed
        || wake.attempts > 0
        || wakes::human_stop_floor(c, &wake.session)? >= wake.id
        || notice.target.stop_epoch != judgments::stop_epoch(c, &wake.session)?
        || !eligible_captain(c, &e.crew, &e.channel, &wake.session)?
    {
        return Ok(work::FollowupNote::Stale);
    }
    let book = asks::load_org(c)?;
    let epoch = judgment_state::epoch(c)?;
    let Some(f) = facts(c, &book, &e.crew, &e.channel)? else {
        return Ok(work::FollowupNote::Stale);
    };
    if !current(&e, &f, &epoch) || !readable_sources(c, &f, &wake.session)? {
        return Ok(work::FollowupNote::Stale);
    }
    let ids = f
        .live
        .iter()
        .map(|w| w.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let prompts = crate::prompts::render_file(crate::settings::locale(c)?);
    Ok(work::FollowupNote::Current(crate::prompts::fill(
        prompts.get("notice.patrol.channel_responsibility"),
        &[("channel", &e.channel), ("work", &ids)],
    )))
}
pub(crate) fn capture_for_claim(tx: &mut Tx<'_>, wake: &wakes::WakeRow) -> Result<()> {
    if !is_notice(&wake.source) {
        return Ok(());
    }
    if !matches!(render(tx.conn(), wake)?, work::FollowupNote::Current(_)) {
        return Err(invalid());
    }
    let mut e = record_for_wake(tx.conn(), wake)?.ok_or_else(invalid)?;
    e.notice.as_mut().ok_or_else(invalid)?.claimed = true;
    save(tx.conn(), e)
}
pub(crate) fn delivered(tx: &mut Tx<'_>, wake: &wakes::WakeRow) -> Result<()> {
    if !is_notice(&wake.source) || wake.state != wakes::WakeState::Inflight || wake.attempts == 0 {
        return Ok(());
    }
    let c = tx.conn();
    c.execute_batch("SAVEPOINT responsibility_delivery")?;
    // Transport has already accepted this batch. Optional notice metadata may
    // be unreadable, but that must not hold unrelated wakes and cursor leases.
    if persist_delivery(c, wake, tx.now_ms()).is_err() {
        c.execute_batch("ROLLBACK TO responsibility_delivery; RELEASE responsibility_delivery")?;
    } else {
        c.execute_batch("RELEASE responsibility_delivery")?;
    }
    Ok(())
}

fn persist_delivery(c: &Connection, wake: &wakes::WakeRow, now: i64) -> Result<()> {
    let Some(mut e) = record_for_wake(c, wake)? else {
        return Ok(());
    };
    let n = e.notice.as_mut().ok_or_else(invalid)?;
    if n.claimed && !n.delivered {
        n.delivered = true;
        e.updated_at = now.max(e.updated_at);
        save(c, e)?;
    }
    Ok(())
}

fn view(c: &Connection, e: Episode, book: &WorkBook, epoch: &str) -> Result<View> {
    if let Some(proof) = &e.handled {
        verify_handled(c, &e, proof)?;
    }
    let mut phase = e.phase;
    if e.epoch != epoch {
        phase = Phase::Stale;
    } else if phase == Phase::Open
        && facts(c, book, &e.crew, &e.channel)?.is_none_or(|f| !current(&e, &f, epoch))
    {
        phase = Phase::Stale;
    }
    let (notice_phase, captain_session_id, wake_id) = if let Some(n) = e.notice {
        let phase = if n.delivered {
            NoticePhase::Delivered
        } else {
            match wakes::by_ids(c, &[n.wake])?.first().filter(|w| owns(&n, w)) {
                Some(w) => match (w.state, n.claimed || w.attempts > 0) {
                    (wakes::WakeState::Pending, false) => NoticePhase::Queued,
                    (wakes::WakeState::Inflight, true) => NoticePhase::Inflight,
                    (wakes::WakeState::Dropped, false) => NoticePhase::Dropped,
                    _ => NoticePhase::Unknown,
                },
                None => NoticePhase::Unknown,
            }
        };
        (phase, Some(n.target.captain), Some(n.wake))
    } else {
        (NoticePhase::NotQueued, None, None)
    };
    Ok(View {
        channel_id: e.channel,
        episode_id: e.id,
        phase,
        notice_phase,
        observed_at_ms: e.observed_at,
        updated_at_ms: e.updated_at,
        work_ids: e.original.into_iter().map(|w| w.id).collect(),
        captain_session_id,
        wake_id,
        handled: (phase == Phase::Handled).then_some(e.handled).flatten(),
    })
}

pub fn read(c: &Connection, p: ChannelResponsibilityParams) -> Result<ChannelResponsibilityPage> {
    crews::get(c, &p.crew_id)?;
    if (p.channel_id.is_some() && p.after.is_some())
        || p.channel_id
            .iter()
            .chain(p.after.iter())
            .any(|id| !valid_channel(id))
    {
        return Err(crate::resource_acl::invalid("channelId"));
    }
    let (rows, next_after) = if let Some(channel) = p.channel_id {
        (load(c, &p.crew_id, &channel)?.into_iter().collect(), None)
    } else {
        page(c, &p.crew_id, p.after.as_deref())?
    };
    let items = if rows.is_empty() {
        Vec::new()
    } else {
        let book = asks::load_org(c)?;
        let epoch = judgment_state::epoch(c)?;
        rows.into_iter()
            .map(|row| view(c, row, &book, &epoch))
            .collect::<Result<Vec<_>>>()?
    };
    Ok(ChannelResponsibilityPage {
        crew_id: p.crew_id,
        items,
        next_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{ok, store},
    };
    use crewd_proto::{WorkAssignParams, WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};

    fn setup(s: &mut crewd_store::Store) -> (String, String) {
        let topic = ok(s, |tx| {
            crate::sessions::set_desired(tx, "cap-a", Desired::Running)?;
            channels::create(
                tx,
                &Sender::Session("cap-a".into()),
                channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "Synthetic responsibility source".into(),
                    purpose: None,
                    members: vec![],
                },
            )
        })
        .id;
        let work = linked(s, &topic, "crew-a");
        (topic, work)
    }
    fn linked(s: &mut crewd_store::Store, topic: &str, crew: &str) -> String {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    channel_id: Some(topic.into()),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    title: "synthetic direct unowned work".into(),
                    ..Default::default()
                },
            )
        })
        .id
    }
    fn one(s: &crewd_store::Store, channel: &str) -> Option<View> {
        read(
            s.conn(),
            ChannelResponsibilityParams {
                crew_id: "crew-a".into(),
                channel_id: Some(channel.into()),
                after: None,
            },
        )
        .unwrap()
        .items
        .pop()
    }
    fn scan(s: &mut crewd_store::Store, topic: &str) {
        ok(s, |tx| observe(tx, "crew-a", &[topic.into()]));
    }

    #[test]
    fn channel_responsibility_delivered_is_distinct_from_actual_handling_and_cannot_replay() {
        let mut s = store();
        let (topic, work_id) = setup(&mut s);
        scan(&mut s, &topic);
        let initial = one(&s, &topic).unwrap();
        assert_eq!(initial.phase, Phase::Open);
        assert_eq!(initial.notice_phase, NoticePhase::NotQueued);
        for _ in 0..3 {
            scan(&mut s, &topic);
            ok(&mut s, |tx| {
                enqueue_next(tx, "crew-a", "cap-a", "same-generation")
            });
        }
        let queued = one(&s, &topic).unwrap();
        assert_eq!(queued.episode_id, initial.episode_id);
        assert_eq!(queued.notice_phase, NoticePhase::Queued);
        let wake = queued.wake_id.unwrap();
        ok(&mut s, |tx| {
            let row = wakes::by_ids(tx.conn(), &[wake])?.remove(0);
            assert!(
                matches!(render(tx.conn(), &row)?, work::FollowupNote::Current(text) if text.contains(&topic) && text.contains(&work_id))
            );
            capture_for_claim(tx, &row)?;
            wakes::claim(tx, &[wake])?;
            wakes::ack_delivered(tx, "cap-a", &[wake], wakes::Riders::Release, &[], &[])
        });
        let delivered = one(&s, &topic).unwrap();
        assert_eq!(delivered.phase, Phase::Open);
        assert_eq!(delivered.notice_phase, NoticePhase::Delivered);
        assert!(delivered.handled.is_none());
        s.conn()
            .execute("DELETE FROM wakes WHERE id=?1", [wake])
            .unwrap();
        assert_eq!(
            one(&s, &topic).unwrap().notice_phase,
            NoticePhase::Delivered
        );
        let p = work::get(s.conn(), &work_id).unwrap();
        ok(&mut s, |tx| {
            work::assign(
                tx,
                &Sender::Human,
                WorkAssignParams {
                    id: p.id,
                    expected_revision: p.revision,
                    owner: "w-a1".into(),
                    note: "ordinary valid claim".into(),
                },
            )
        });
        let result = one(&s, &topic).unwrap();
        assert_eq!(result.phase, Phase::Handled);
        assert_eq!(result.episode_id, initial.episode_id);
        let proof = result.handled.unwrap();
        assert_eq!(proof.work_id, work_id);
        assert_eq!(proof.owner_session_id, "w-a1");
        scan(&mut s, &topic);
        ok(&mut s, |tx| {
            enqueue_next(tx, "crew-a", "cap-a", "different-generation")
        });
        assert_eq!(one(&s, &topic).unwrap().wake_id, Some(wake));
    }

    #[test]
    fn channel_responsibility_corrupt_pending_notice_has_no_render_or_claim() {
        let mut s = store();
        let (topic, _) = setup(&mut s);
        scan(&mut s, &topic);
        ok(&mut s, |tx| {
            enqueue_next(tx, "crew-a", "cap-a", "generation-1")
        });
        let notice = one(&s, &topic).unwrap().wake_id.unwrap();
        meta_set(s.conn(), &key("crew-a", &topic), "invalid sealed episode").unwrap();
        let row = wakes::by_ids(s.conn(), &[notice]).unwrap().remove(0);
        assert!(matches!(
            render(s.conn(), &row).unwrap(),
            work::FollowupNote::Stale
        ));
        crate::testkit::err(&mut s, |tx| capture_for_claim(tx, &row));
        let stored = wakes::by_ids(s.conn(), &[notice]).unwrap().remove(0);
        assert_eq!(stored.state, wakes::WakeState::Pending);
        assert_eq!(stored.attempts, 0);
    }

    #[test]
    fn channel_responsibility_receipt_fault_does_not_hold_a_mixed_delivery_batch() {
        let mut s = store();
        let (topic, _) = setup(&mut s);
        scan(&mut s, &topic);
        ok(&mut s, |tx| {
            enqueue_next(tx, "crew-a", "cap-a", "generation-1")
        });
        let notice = one(&s, &topic).unwrap().wake_id.unwrap();
        let wakes::Enqueued::New(ordinary) = ok(&mut s, |tx| {
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: "cap-a".into(),
                    kind: wakes::WakeKind::System,
                    source: "synthetic ordinary delivery".into(),
                    priority: wakes::WakePriority::Auto,
                    not_before_ms: None,
                    note: None,
                    origin: None,
                },
            )
        }) else {
            panic!("ordinary wake must be new");
        };
        ok(&mut s, |tx| {
            let row = wakes::by_ids(tx.conn(), &[notice])?.remove(0);
            capture_for_claim(tx, &row)?;
            wakes::claim(tx, &[notice, ordinary])
        });
        s.conn()
            .execute_batch(
                "CREATE TEMP TRIGGER deny_responsibility_receipt BEFORE INSERT ON meta
             WHEN NEW.key LIKE 'channel.responsibility.v1:%'
             BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;",
            )
            .unwrap();
        ok(&mut s, |tx| {
            wakes::ack_delivered(
                tx,
                "cap-a",
                &[notice, ordinary],
                wakes::Riders::Release,
                &[],
                &[],
            )
        });
        let settled = wakes::by_ids(s.conn(), &[notice, ordinary]).unwrap();
        assert_eq!(settled.len(), 2);
        assert!(
            settled
                .iter()
                .all(|row| row.state == wakes::WakeState::Delivered)
        );
        assert_eq!(one(&s, &topic).unwrap().notice_phase, NoticePhase::Unknown);
        ok(&mut s, |tx| {
            enqueue_next(tx, "crew-a", "cap-a", "different-generation")
        });
        assert_eq!(one(&s, &topic).unwrap().wake_id, Some(notice));
    }

    #[test]
    fn channel_responsibility_refuses_foreign_and_restored_sources_and_distinguishes_ending() {
        let mut s = store();
        let (topic, _) = setup(&mut s);
        scan(&mut s, &topic);
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        scan(&mut s, &topic);
        ok(&mut s, |tx| {
            enqueue_next(tx, "crew-a", "cap-a", "new-generation")
        });
        let stale = one(&s, &topic).unwrap();
        assert_eq!(stale.phase, Phase::Stale);
        assert_eq!(stale.notice_phase, NoticePhase::NotQueued);
        linked(&mut s, &topic, "crew-a");
        scan(&mut s, &topic);
        ok(&mut s, |tx| {
            enqueue_next(tx, "crew-a", "cap-a", "new-generation")
        });
        let still_stale = one(&s, &topic).unwrap();
        assert_eq!(still_stale.episode_id, stale.episode_id);
        assert_eq!(still_stale.notice_phase, NoticePhase::NotQueued);
        let mut s = store();
        let (topic, _) = setup(&mut s);
        linked(&mut s, &topic, "crew-b");
        scan(&mut s, &topic);
        assert!(one(&s, &topic).is_none());
        let mut s = store();
        let (topic, id) = setup(&mut s);
        scan(&mut s, &topic);
        let plan = work::get(s.conn(), &id).unwrap();
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id,
                    expected_revision: plan.revision,
                    status: Some(WorkUpdateStatus::Dropped),
                    note: Some("ended without assignment".into()),
                    ..Default::default()
                },
            )
        });
        scan(&mut s, &topic);
        let ended = one(&s, &topic).unwrap();
        assert_eq!(ended.phase, Phase::Ended);
        assert!(ended.handled.is_none());
        let successor = linked(&mut s, &topic, "crew-a");
        scan(&mut s, &topic);
        let reopened = one(&s, &topic).unwrap();
        assert_ne!(reopened.episode_id, ended.episode_id);
        assert_eq!(reopened.work_ids, vec![successor]);
    }
}
