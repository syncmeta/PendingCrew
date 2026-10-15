//! Authoritative bounded inputs for the optional judgment service. This module
//! gathers facts only; model output is not an Actor or a WorkBook transition.
use crate::{Result, channels, crews, messages, requirements, sessions, wakes};
use crewd_proto::Role;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipient {
    pub id: String,
    pub name: String,
    pub captain: bool,
    pub stop_floor: i64,
    pub stop_epoch: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkCandidate {
    pub kind: String,
    pub id: String,
    pub summary: String,
    pub revision: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputSnapshot {
    pub kind: String,
    pub id: String,
    pub crew: String,
    pub channel: Option<String>,
    pub text: String,
    pub context_version: String,
    pub captain: Option<String>,
    pub recipients: Vec<Recipient>,
    pub links: Vec<LinkCandidate>,
    pub version: String,
}
pub fn fingerprint(v: &impl Serialize) -> Result<String> {
    let data = serde_json::to_vec(v).map_err(crewd_store::StoreError::from)?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn recipients(c: &Connection, crew: &str, channel: Option<&str>) -> Result<Vec<Recipient>> {
    let captain = crews::captain_of(c, crew)?;
    let mut all = sessions::list(c, Some(crew))?;
    all.sort_by(|a, b| {
        (captain.as_ref() == Some(&b.id))
            .cmp(&(captain.as_ref() == Some(&a.id)))
            .then(b.last_active_at.cmp(&a.last_active_at))
            .then(a.id.cmp(&b.id))
    });
    let mut out = Vec::new();
    for s in all {
        if s.resume_failed_at.is_some() || !matches!(s.role, Role::Captain | Role::Worker) {
            continue;
        }
        if let Some(ch) = channel
            && !channels::is_member(c, ch, &crewd_proto::MemberRef::session(s.id.clone()))?
        {
            continue;
        }
        let stop_floor = wakes::human_stop_floor(c, &s.id)?;
        out.push(Recipient {
            id: s.id.clone(),
            name: s.display_name.unwrap_or_else(|| "member".into()),
            captain: captain.as_ref() == Some(&s.id),
            stop_floor,
            stop_epoch: stop_epoch(c, &s.id)?,
        });
    }
    // Same existing bounded wake candidate contract; the business layer applies
    // its stricter route/attribute bounds, rather than inventing another cap.
    out.truncate(30);
    Ok(out)
}
fn finish(mut s: InputSnapshot) -> Result<InputSnapshot> {
    s.version = fingerprint(&s)?;
    Ok(s)
}
pub fn requirement_input(c: &Connection, id: &str) -> Result<Option<InputSnapshot>> {
    let d = requirements::get(c, id)?;
    if d.todo.status != "unclaimed"
        || d.closure.is_some()
        || crews::is_archived(c, &d.todo.crew_id)?
    {
        return Ok(None);
    }
    let extra: Option<String> =
        c.query_row("SELECT extra FROM todos WHERE id=?1", [&d.todo.id], |r| {
            r.get(0)
        })?;
    let marker: Value = extra
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(crewd_store::StoreError::from)?
        .unwrap_or(Value::Null);
    let channel = marker
        .get("sourceChannelId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(ch) = &channel {
        let row = channels::get(c, ch)?;
        if row.crew_id != d.todo.crew_id || row.archived_at.is_some() {
            return Ok(None);
        }
    }
    finish(InputSnapshot {
        kind: "route".into(),
        id: id.into(),
        crew: d.todo.crew_id.clone(),
        channel: channel.clone(),
        text: d.todo.text,
        context_version: match &channel {
            Some(ch) => fingerprint(&channels::get(c, ch)?)?,
            None => String::new(),
        },
        captain: crews::captain_of(c, &d.todo.crew_id)?,
        recipients: recipients(c, &d.todo.crew_id, channel.as_deref())?,
        links: vec![],
        version: String::new(),
    })
    .map(Some)
}
fn listen_revision(c: &Connection, channel: &str) -> Result<String> {
    let mut q = c.prepare(
        "SELECT session_id,until,senders FROM listens WHERE channel_id=?1 ORDER BY session_id",
    )?;
    let rows = q
        .query_map([channel], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    fingerprint(&rows)
}
pub fn message_input(c: &Connection, id: &str) -> Result<Option<InputSnapshot>> {
    let m = messages::get(c, id)?;
    if m.sender_kind != crewd_proto::SenderKind::Human {
        return Ok(None);
    }
    let ch = channels::get(c, &m.channel_id)?;
    if ch.archived_at.is_some() || crews::is_archived(c, &ch.crew_id)? {
        return Ok(None);
    }
    let page = crate::channel_work::list(
        c,
        &crate::Sender::Human,
        crewd_proto::ChannelWorkListParams {
            channel_id: m.channel_id.clone(),
            after: None,
            limit: Some(10),
        },
    )?;
    let mut links = Vec::new();
    for item in page.items {
        let kind = match item.object.kind {
            crewd_proto::ChannelWorkKind::Requirement => "requirement",
            crewd_proto::ChannelWorkKind::Work => "work",
            crewd_proto::ChannelWorkKind::HumanTask => continue,
        };
        if item.crew_id != ch.crew_id
            || matches!(item.status.as_str(), "done" | "dropped" | "closed")
        {
            continue;
        }
        links.push(LinkCandidate {
            kind: kind.into(),
            id: item.object.id,
            summary: item.label,
            revision: format!(
                "{}:{}:{}",
                page.link_revision,
                item.status,
                item.revision.unwrap_or_default()
            ),
        });
    }

    // Only this same channel's prior messages enter the model context. No private
    // sibling channel text or human task answer crosses a visibility boundary.
    let mut q=c.prepare("SELECT id,text,seq FROM messages WHERE channel_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 10")?;
    for row in q.query_map(rusqlite::params![m.channel_id, m.seq], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (id, text, seq) = row?;
        links.push(LinkCandidate {
            kind: "message".into(),
            id,
            summary: text.chars().take(200).collect(),
            revision: seq.to_string(),
        });
    }
    finish(InputSnapshot {
        kind: "message".into(),
        id: id.into(),
        crew: ch.crew_id.clone(),
        channel: Some(m.channel_id.clone()),
        context_version: fingerprint(&(
            &ch,
            &m,
            listen_revision(c, &m.channel_id)?,
            // Capture the association revision even when the visible candidate
            // page becomes empty, so unlink/relink cannot resurrect an old cut.
            &page.link_revision,
        ))?,
        text: m.text,
        captain: crews::captain_of(c, &ch.crew_id)?,
        recipients: recipients(c, &ch.crew_id, Some(&m.channel_id))?,
        links,
        version: String::new(),
    })
    .map(Some)
}
/// Read named projection fields without treating malformed/unknown versions as
/// absence. The eventual settlement DAO owns writes to these existing JSON rows.
pub fn projection(c: &Connection, kind: &str, id: &str) -> Result<Option<Value>> {
    let raw: Option<String> = match kind {
        "message" => c
            .query_row("SELECT extra FROM messages WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .flatten(),
        "requirement" => {
            let d = requirements::get(c, id)?;
            c.query_row("SELECT extra FROM todos WHERE id=?1", [d.todo.id], |r| {
                r.get(0)
            })?
        }
        _ => {
            return Err(
                crewd_store::StoreError::Invalid("invalid judgment source kind".into()).into(),
            );
        }
    };
    raw.map(|v| {
        serde_json::from_str(&v)
            .map_err(crewd_store::StoreError::from)
            .map_err(Into::into)
    })
    .transpose()
}

pub const CONFIG_KEY: &str = "judgment.config.v1";
pub const PENDING_KEY: &str = "judgment.secret.pending.v1";
pub const ACTIVE_KEY: &str = "judgment.active.v1";
const RECORD_KEY: &str = "judgmentV1";
use crate::{Change, CollabError, Sender, Tx};
use crewd_proto::judgments::*;
fn invalid(field: &str) -> CollabError {
    CollabError::Invalid(
        crate::refusal::Refusal::new(crate::refusal::Code::ResourceInvalid).arg("field", field),
    )
}
pub fn configuration(c: &Connection) -> Result<JudgmentSettings> {
    crewd_store::meta_get(c, CONFIG_KEY)?
        .map(|v| {
            serde_json::from_str(&v)
                .map_err(crewd_store::StoreError::from)
                .map_err(Into::into)
        })
        .transpose()
        .map(|v| {
            v.unwrap_or(JudgmentSettings {
                revision: None,
                config: None,
            })
        })
}
pub fn set_configuration(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: JudgmentSettingsSet,
) -> Result<JudgmentSettings> {
    if !matches!(by, Sender::Human) {
        return Err(CollabError::Forbidden(crate::refusal::Refusal::new(
            crate::refusal::Code::ResourceForbidden,
        )));
    }
    let old = configuration(tx.conn())?;
    if old.revision != p.expected_revision {
        return Err(CollabError::Conflict(
            crate::refusal::Refusal::new(crate::refusal::Code::ResourceRequestConflict)
                .arg("request", "judgment.settings"),
        ));
    }
    if let Some(c) = &p.config
        && (c.provider != "typesafe"
            || c.model.trim().is_empty()
            || !matches!(c.credential_source.as_str(), "platform" | "file")
            || [c.choice_min, c.wake_yes]
                .into_iter()
                .any(|n| !n.is_finite() || !(0.0..=1.0).contains(&n)))
    {
        return Err(invalid("judgment.config"));
    }
    let next = JudgmentSettings {
        revision: Some(uuid::Uuid::new_v4().to_string()),
        config: p.config,
    };
    crewd_store::meta_set(
        tx.conn(),
        CONFIG_KEY,
        &serde_json::to_string(&next).map_err(crewd_store::StoreError::from)?,
    )?;
    Ok(next)
}
fn key(kind: &JudgmentSourceKind) -> &'static str {
    match kind {
        JudgmentSourceKind::Message => "message",
        JudgmentSourceKind::Requirement => "requirement",
    }
}
fn extra(c: &Connection, kind: &JudgmentSourceKind, id: &str) -> Result<Value> {
    Ok(projection(c, key(kind), id)?.unwrap_or(serde_json::json!({})))
}
pub fn record(
    c: &Connection,
    kind: &JudgmentSourceKind,
    id: &str,
) -> Result<Option<JudgmentRecord>> {
    let raw = extra(c, kind, id)?;
    let Some(v) = raw.get(RECORD_KEY) else {
        return Ok(None);
    };
    let mut row: JudgmentRecord =
        serde_json::from_value(v.clone()).map_err(crewd_store::StoreError::from)?;
    if row.version != 1
        || row.decision_id.is_empty()
        || row.db_id.is_empty()
        || row.captured_floors.values().any(|n| *n < 0)
    {
        return Err(
            crewd_store::StoreError::Invalid("unknown/corrupt judgment projection".into()).into(),
        );
    }
    if crewd_store::meta_get(c, "dbId")?.as_deref() != Some(row.db_id.as_str())
        || crewd_store::judgment_state::epoch(c)? != row.restore_epoch
    {
        row.phase = JudgmentPhase::Historical;
    }
    Ok(Some(row))
}
fn put(tx: &mut Tx<'_>, kind: &JudgmentSourceKind, id: &str, row: &JudgmentRecord) -> Result<()> {
    let mut raw = extra(tx.conn(), kind, id)?;
    if raw.is_null() {
        raw = serde_json::json!({})
    }
    let object = raw.as_object_mut().ok_or_else(|| {
        crewd_store::StoreError::Invalid("source metadata is not an object".into())
    })?;
    object.insert(
        RECORD_KEY.into(),
        serde_json::to_value(row).map_err(crewd_store::StoreError::from)?,
    );
    let value = serde_json::to_string(&raw).map_err(crewd_store::StoreError::from)?;
    match kind {
        JudgmentSourceKind::Message => {
            tx.conn().execute(
                "UPDATE messages SET extra=?1 WHERE id=?2",
                rusqlite::params![value, id],
            )?;
        }
        JudgmentSourceKind::Requirement => {
            let d = requirements::get(tx.conn(), id)?;
            tx.conn().execute(
                "UPDATE todos SET extra=?1 WHERE id=?2",
                rusqlite::params![value, d.todo.id],
            )?;
        }
    };
    Ok(())
}
pub fn input(c: &Connection, kind: &JudgmentSourceKind, id: &str) -> Result<Option<InputSnapshot>> {
    match kind {
        JudgmentSourceKind::Message => message_input(c, id),
        JudgmentSourceKind::Requirement => requirement_input(c, id),
    }
}
/// A unique cancellation epoch invalidates optional judgments even when Stop
/// precedes every wake ID. Stop never parses optional provider metadata.
pub fn stop_epoch(c: &Connection, session: &str) -> Result<String> {
    Ok(crewd_store::meta_get(c, &format!("judgment.stop.v1:{session}"))?.unwrap_or_default())
}
pub(crate) fn note_stop(tx: &mut Tx<'_>, session: &str) -> Result<()> {
    if crewd_store::meta_get(tx.conn(), CONFIG_KEY)?.is_some() {
        crewd_store::meta_set(
            tx.conn(),
            &format!("judgment.stop.v1:{session}"),
            &uuid::Uuid::new_v4().to_string(),
        )?;
    }
    Ok(())
}
fn epochs(
    c: &Connection,
    ids: &std::collections::BTreeMap<String, i64>,
) -> Result<std::collections::BTreeMap<String, String>> {
    ids.keys()
        .map(|id| stop_epoch(c, id).map(|n| (id.clone(), n)))
        .collect()
}
fn floors(
    c: &Connection,
    s: &InputSnapshot,
    now: i64,
) -> Result<std::collections::BTreeMap<String, i64>> {
    let mut ids: Vec<_> = s.recipients.iter().map(|r| r.id.clone()).collect();
    if let Some(ch) = &s.channel {
        ids.extend(
            channels::members(c, ch)?
                .into_iter()
                .filter_map(|m| match m {
                    crewd_proto::MemberRef::Session { id } => Some(id),
                    _ => None,
                }),
        );
        ids.extend(
            crate::listens::active_for_channel(c, ch, now)?
                .into_iter()
                .map(|l| l.session),
        );
    }
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| wakes::human_stop_floor(c, &id).map(|f| (id, f)))
        .collect()
}
/// Called only alongside creation of an immutable human source fact. Old rows
/// are never auto-enrolled by a timer, event replay or enabling the setting.
pub fn reserve(tx: &mut Tx<'_>, kind: JudgmentSourceKind, id: &str) -> Result<bool> {
    if crewd_store::meta_get(tx.conn(), ACTIVE_KEY)?.is_none()
        || crewd_store::meta_get(tx.conn(), PENDING_KEY)?.is_some()
    {
        return Ok(false);
    }
    let Ok(settings) = configuration(tx.conn()) else {
        return Ok(false);
    };
    if !settings.config.as_ref().is_some_and(|c| c.enabled) {
        return Ok(false);
    }
    if record(tx.conn(), &kind, id)?.is_some() {
        return Ok(true);
    }
    let Some(s) = input(tx.conn(), &kind, id)? else {
        return Ok(false);
    };
    let captured_floors = floors(tx.conn(), &s, tx.now_ms())?;
    let captured_stop_epochs = epochs(tx.conn(), &captured_floors)?;
    let r = JudgmentRecord {
        version: 1,
        decision_id: uuid::Uuid::new_v4().to_string(),
        db_id: crewd_store::meta_get(tx.conn(), "dbId")?.ok_or_else(|| invalid("dbId"))?,
        restore_epoch: crewd_store::judgment_state::epoch(tx.conn())?,
        config_revision: settings
            .revision
            .ok_or_else(|| invalid("judgment.revision"))?,
        source_version: s.version.clone(),
        phase: JudgmentPhase::Pending,
        prepared_at_ms: tx.now_ms(),
        captured_floors,
        captured_stop_epochs,
        recipients: vec![],
        attribution: None,
        diagnostic: serde_json::json!({"status":"prepared"}),
    };
    put(tx, &kind, id, &r)?;
    Ok(true)
}
pub fn dispatch(
    tx: &mut Tx<'_>,
    kind: &JudgmentSourceKind,
    id: &str,
) -> Result<Option<(JudgmentRecord, InputSnapshot, JudgmentSettings)>> {
    let Some(mut r) = record(tx.conn(), kind, id)? else {
        return Ok(None);
    };
    if r.phase != JudgmentPhase::Pending {
        return Ok(None);
    }
    let Some(s) = input(tx.conn(), kind, id)? else {
        r.phase = JudgmentPhase::Suppressed;
        put(tx, kind, id, &r)?;
        return Ok(None);
    };
    let settings = configuration(tx.conn())?;
    if crewd_store::meta_get(tx.conn(), "dbId")?.as_deref() != Some(r.db_id.as_str()) {
        r.phase = JudgmentPhase::Historical;
        put(tx, kind, id, &r)?;
        return Ok(None);
    }
    if s.version != r.source_version
        || settings.revision.as_ref() != Some(&r.config_revision)
        || crewd_store::meta_get(tx.conn(), PENDING_KEY)?.is_some()
        || floors(tx.conn(), &s, tx.now_ms())? != r.captured_floors
        || epochs(tx.conn(), &r.captured_floors)? != r.captured_stop_epochs
    {
        r.phase = JudgmentPhase::Suppressed;
        r.diagnostic = serde_json::json!({"status":"stale"});
        put(tx, kind, id, &r)?;
        return Ok(None);
    }
    r.phase = JudgmentPhase::Dispatched;
    put(tx, kind, id, &r)?;
    Ok(Some((r, s, settings)))
}
pub fn inspect(c: &Connection, by: &Sender, p: JudgmentInspect) -> Result<Option<JudgmentRecord>> {
    let (crew, channel) = match p.kind {
        JudgmentSourceKind::Requirement => {
            let d = requirements::get(c, &p.id)?;
            (d.todo.crew_id, None)
        }
        JudgmentSourceKind::Message => {
            let m = messages::get(c, &p.id)?;
            let ch = channels::get(c, &m.channel_id)?;
            (ch.crew_id, Some(m.channel_id))
        }
    };
    if crew != p.crew_id {
        return Err(invalid("crewId"));
    }
    match by {
        Sender::Human => {}
        Sender::Session(sid) => {
            let me = sessions::get(c, sid)?;
            if me.crew_id != crew {
                return Err(CollabError::Forbidden(crate::refusal::Refusal::new(
                    crate::refusal::Code::ResourceForbidden,
                )));
            }
            if let Some(ch) = channel
                && !channels::readable_by(c, &ch, sid)?
            {
                return Err(CollabError::Forbidden(crate::refusal::Refusal::new(
                    crate::refusal::Code::ResourceForbidden,
                )));
            }
        }
        Sender::System => {
            return Err(CollabError::Forbidden(crate::refusal::Refusal::new(
                crate::refusal::Code::ResourceForbidden,
            )));
        }
    }
    record(c, &p.kind, &p.id)
}
/// Current authority, caller-provided proposal and durable attribution share one
/// transaction. A proposal cannot authorize an owner, reply, Ask or external tool.
pub fn settle(
    tx: &mut Tx<'_>,
    kind: &JudgmentSourceKind,
    id: &str,
    decision: &str,
    recipients: Vec<String>,
    attribution: Option<JudgmentLink>,
    diagnostic: Value,
) -> Result<bool> {
    let Some(mut r) = record(tx.conn(), kind, id)? else {
        return Ok(false);
    };
    if r.decision_id != decision
        || !matches!(r.phase, JudgmentPhase::Pending | JudgmentPhase::Dispatched)
    {
        return Ok(false);
    }
    let db = crewd_store::meta_get(tx.conn(), "dbId")?;
    if db.as_deref() != Some(&r.db_id) {
        r.phase = JudgmentPhase::Historical;
        put(tx, kind, id, &r)?;
        return Ok(false);
    }
    let settings = configuration(tx.conn())?;
    let current = input(tx.conn(), kind, id)?;
    let valid = current
        .as_ref()
        .is_some_and(|s| s.version == r.source_version)
        && settings.revision.as_ref() == Some(&r.config_revision);
    let valid = valid
        && crewd_store::meta_get(tx.conn(), PENDING_KEY)?.is_none()
        && epochs(tx.conn(), &r.captured_floors)? == r.captured_stop_epochs
        && current
            .as_ref()
            .map(|s| floors(tx.conn(), s, tx.now_ms()))
            .transpose()?
            .as_ref()
            == Some(&r.captured_floors);
    if !valid {
        r.phase = JudgmentPhase::Suppressed;
        r.diagnostic = serde_json::json!({"status":"stale"});
        put(tx, kind, id, &r)?;
        return Ok(false);
    }
    let s = current.expect("checked current snapshot");
    if recipients
        .iter()
        .any(|id| !r.captured_floors.contains_key(id))
    {
        return Err(invalid("judgment.recipient"));
    }
    if let Some(link) = &attribution
        && !s
            .links
            .iter()
            .any(|c| c.kind == link.kind.as_key() && c.id == link.id && c.revision == link.revision)
    {
        return Err(invalid("judgment.attribution"));
    }
    r.phase = JudgmentPhase::Settled;
    r.recipients = recipients;
    r.attribution = attribution;
    r.diagnostic = diagnostic;
    put(tx, kind, id, &r)?;
    if *kind == JudgmentSourceKind::Requirement {
        tx.note(Change::WorkChanged {
            crew_id: s.crew,
            id: id.into(),
        });
    }
    Ok(true)
}
/// Rendering/claim repeat these checks after queueing; a durable route does not
/// override a later Stop, role/membership change, archive or requirement closure.
pub(crate) fn route_note(
    c: &Connection,
    row: &wakes::WakeRow,
) -> Result<crate::work::FollowupNote> {
    let Some(id) = row.source.strip_prefix("judgment-route:") else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    let Some(r) = record(c, &JudgmentSourceKind::Requirement, id)? else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    let Some(s) = requirement_input(c, id)? else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    if r.phase != JudgmentPhase::Settled
        || !r.recipients.contains(&row.session)
        || !s.recipients.iter().any(|m| m.id == row.session)
        || r.captured_stop_epochs.get(&row.session) != Some(&stop_epoch(c, &row.session)?)
        || r.captured_floors.get(&row.session) != Some(&wakes::human_stop_floor(c, &row.session)?)
        || row.id <= *r.captured_floors.get(&row.session).unwrap_or(&i64::MAX)
        || crewd_store::meta_get(c, "dbId")?.as_deref() != Some(r.db_id.as_str())
    {
        return Ok(crate::work::FollowupNote::Stale);
    }
    Ok(crate::work::FollowupNote::Current(
        serde_json::json!({"newRequirement":id,"crewId":s.crew,"decisionId":r.decision_id})
            .to_string(),
    ))
}

/// Internal restart-only transition. It settles an unknown admitted attempt without
/// issuing another provider call; source/config/db checks still occur in settle.
pub fn restore_pending_for_settlement(
    tx: &mut Tx<'_>,
    kind: &JudgmentSourceKind,
    id: &str,
    record: &JudgmentRecord,
) -> Result<()> {
    put(tx, kind, id, record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};
    fn configured(s: &mut crewd_store::Store) {
        let settings = JudgmentSettings {
            revision: Some("fake-config".into()),
            config: Some(JudgmentConfig {
                enabled: true,
                provider: "typesafe".into(),
                base_url: "http://127.0.0.1:1".into(),
                model: "fake".into(),
                choice_min: 0.5,
                wake_yes: 0.5,
                credential_source: "file".into(),
            }),
        };
        crewd_store::meta_set(
            s.conn(),
            CONFIG_KEY,
            &serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        crewd_store::meta_set(s.conn(), ACTIVE_KEY, "fake-runtime").unwrap();
    }
    fn requirement(s: &mut crewd_store::Store) -> String {
        ok(s, |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: None,
                    text: "immutable human requirement".into(),
                },
            )
        })
        .todo
        .requirement_id
        .unwrap()
    }
    #[test]
    fn stopped_member_new_human_route_settles_once_without_creating_work_or_mutating_source() {
        let mut s = store();
        configured(&mut s);
        let id = requirement(&mut s);
        let (r, input, _) = ok(&mut s, |tx| {
            dispatch(tx, &JudgmentSourceKind::Requirement, &id)
        })
        .unwrap();
        assert!(input.recipients.iter().any(|m| m.id == "w-a1"));
        let before = requirements::get(s.conn(), &id).unwrap().todo;
        let extra: Option<String> = s
            .conn()
            .query_row("SELECT extra FROM todos WHERE id=?1", [&before.id], |r| {
                r.get(0)
            })
            .unwrap();
        let mut value: Value = serde_json::from_str(extra.as_deref().unwrap()).unwrap();
        value["futurePrivateField"] = serde_json::json!({"unchanged":true});
        s.conn()
            .execute(
                "UPDATE todos SET extra=?1 WHERE id=?2",
                rusqlite::params![value.to_string(), before.id],
            )
            .unwrap();
        assert!(ok(&mut s, |tx| settle(
            tx,
            &JudgmentSourceKind::Requirement,
            &id,
            &r.decision_id,
            vec!["w-a1".into()],
            None,
            serde_json::json!({"status":"answered"})
        )));
        assert!(!ok(&mut s, |tx| settle(
            tx,
            &JudgmentSourceKind::Requirement,
            &id,
            &r.decision_id,
            vec!["w-a1".into()],
            None,
            Value::Null
        )));
        assert!(
            ok(&mut s, |tx| dispatch(
                tx,
                &JudgmentSourceKind::Requirement,
                &id
            ))
            .is_none()
        );
        let after = requirements::get(s.conn(), &id).unwrap().todo;
        assert_eq!(before.text, after.text);
        assert_eq!(after.status, "unclaimed");
        assert_eq!(
            projection(s.conn(), "requirement", &id).unwrap().unwrap()["futurePrivateField"],
            serde_json::json!({"unchanged":true})
        );
    }
    #[test]
    fn authoritative_plan_acceptance_suppresses_an_inflight_route_without_changing_raw_todo() {
        let mut s = store();
        configured(&mut s);
        let id = requirement(&mut s);
        let (r, _, _) = ok(&mut s, |tx| {
            dispatch(tx, &JudgmentSourceKind::Requirement, &id)
        })
        .unwrap();
        let raw_id = requirements::get(s.conn(), &id).unwrap().todo.id;
        let before: String = s
            .conn()
            .query_row("SELECT status FROM todos WHERE id=?1", [&raw_id], |row| {
                row.get(0)
            })
            .unwrap();
        let plan = ok(&mut s, |tx| {
            crate::work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "human accepted the requirement".into(),
                    owner: Some("w-a2".into()),
                    sources: vec![id.clone()],
                    ..Default::default()
                },
            )
        });
        assert_eq!(plan.owner.as_deref(), Some("w-a2"));
        // The source row remains unchanged: authority is the WorkBook-derived
        // requirement view, never the legacy todos.status column alone.
        let after: String = s
            .conn()
            .query_row("SELECT status FROM todos WHERE id=?1", [&raw_id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            requirements::get(s.conn(), &id).unwrap().todo.status,
            "in_progress"
        );
        assert!(requirement_input(s.conn(), &id).unwrap().is_none());
        assert!(!ok(&mut s, |tx| settle(
            tx,
            &JudgmentSourceKind::Requirement,
            &id,
            &r.decision_id,
            vec!["w-a1".into()],
            None,
            Value::Null
        )));
        let record = record(s.conn(), &JudgmentSourceKind::Requirement, &id)
            .unwrap()
            .unwrap();
        assert_eq!(record.phase, JudgmentPhase::Suppressed);
        assert!(record.recipients.is_empty());
        assert_eq!(
            crate::work::get(s.conn(), &plan.id)
                .unwrap()
                .owner
                .as_deref(),
            Some("w-a2")
        );
    }

    #[test]
    fn stop_without_any_wake_then_resume_cannot_resurrect_a_dispatched_decision() {
        let mut s = store();
        configured(&mut s);
        let id = requirement(&mut s);
        let (r, _, _) = ok(&mut s, |tx| {
            dispatch(tx, &JudgmentSourceKind::Requirement, &id)
        })
        .unwrap();
        assert_eq!(wakes::human_stop_floor(s.conn(), "w-a1").unwrap(), 0);
        ok(&mut s, |tx| wakes::human_stop(tx, "w-a1"));
        ok(&mut s, |tx| {
            sessions::set_desired(tx, "w-a1", crewd_proto::Desired::Running)
        });
        assert!(!ok(&mut s, |tx| settle(
            tx,
            &JudgmentSourceKind::Requirement,
            &id,
            &r.decision_id,
            vec!["w-a1".into()],
            None,
            Value::Null
        )));
        assert_eq!(
            record(s.conn(), &JudgmentSourceKind::Requirement, &id)
                .unwrap()
                .unwrap()
                .phase,
            JudgmentPhase::Suppressed
        );
    }
    #[test]
    fn configuration_barrier_prevents_dispatch_and_restore_changes_history_without_replay() {
        let mut s = store();
        configured(&mut s);
        let id = requirement(&mut s);
        crewd_store::meta_set(s.conn(), PENDING_KEY, "unknown-operation").unwrap();
        assert!(
            ok(&mut s, |tx| dispatch(
                tx,
                &JudgmentSourceKind::Requirement,
                &id
            ))
            .is_none()
        );
        assert_eq!(
            record(s.conn(), &JudgmentSourceKind::Requirement, &id)
                .unwrap()
                .unwrap()
                .phase,
            JudgmentPhase::Suppressed
        );
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [PENDING_KEY])
            .unwrap();
        let restored = requirement(&mut s);
        crewd_store::meta_set(s.conn(), "dbId", "different-db").unwrap();
        assert!(
            ok(&mut s, |tx| dispatch(
                tx,
                &JudgmentSourceKind::Requirement,
                &restored
            ))
            .is_none()
        );
        assert_eq!(
            record(s.conn(), &JudgmentSourceKind::Requirement, &restored)
                .unwrap()
                .unwrap()
                .phase,
            JudgmentPhase::Historical
        );
    }
    #[test]
    fn message_link_close_unlink_and_membership_changes_invalidate_inflight_attribution() {
        for change in ["close", "unlink", "membership"] {
            let mut s = store();
            configured(&mut s);
            let channel = ok(&mut s, |tx| {
                channels::create(
                    tx,
                    &Sender::Human,
                    channels::NewChannel {
                        crew_id: "crew-a".into(),
                        kind: crewd_proto::ChannelKind::Topic,
                        name: "private test topic".into(),
                        members: vec![
                            crewd_proto::MemberRef::session("cap-a"),
                            crewd_proto::MemberRef::session("w-a1"),
                        ],
                        ..Default::default()
                    },
                )
            })
            .id;
            let requirement = ok(&mut s, |tx| {
                requirements::create(
                    tx,
                    &Sender::Human,
                    crewd_proto::TodoCreateParams {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        crew_id: "crew-a".into(),
                        channel_id: Some(channel.clone()),
                        text: "source fact".into(),
                    },
                )
            })
            .todo
            .requirement_id
            .unwrap();
            let message = ok(&mut s, |tx| {
                messages::post(
                    tx,
                    Sender::Human,
                    messages::NewPost {
                        channel_id: channel.clone(),
                        text: "about source".into(),
                        ..Default::default()
                    },
                )
            })
            .message;
            let (r, input, _) = ok(&mut s, |tx| {
                dispatch(tx, &JudgmentSourceKind::Message, &message.id)
            })
            .unwrap();
            let link = input
                .links
                .iter()
                .find(|l| l.id == requirement)
                .unwrap()
                .clone();
            match change {
                "close" => {
                    ok(&mut s, |tx| {
                        requirements::close(
                            tx,
                            &Sender::Human,
                            crewd_proto::TodoCloseParams {
                                id: requirement.clone(),
                                expected_status: "unclaimed".into(),
                                note: "human closed".into(),
                            },
                        )
                    });
                }
                "unlink" => {
                    let rev = crewd_store::channel_work::revision(s.conn(), &channel).unwrap();
                    ok(&mut s, |tx| {
                        crate::channel_work::link(
                            tx,
                            &Sender::Human,
                            crewd_proto::ChannelWorkLinkParams {
                                channel_id: channel.clone(),
                                expected_link_revision: rev.to_string(),
                                object: crewd_proto::ChannelWorkObject {
                                    kind: crewd_proto::ChannelWorkKind::Requirement,
                                    id: requirement.clone(),
                                },
                            },
                            false,
                        )
                    });
                }
                _ => {
                    ok(&mut s, |tx| {
                        channels::remove_member(
                            tx,
                            &Sender::Human,
                            &channel,
                            &crewd_proto::MemberRef::session("w-a1"),
                        )
                    });
                }
            }
            assert!(
                !ok(&mut s, |tx| settle(
                    tx,
                    &JudgmentSourceKind::Message,
                    &message.id,
                    &r.decision_id,
                    vec!["cap-a".into()],
                    Some(JudgmentLink {
                        kind: JudgmentObjectKind::Requirement,
                        id: link.id.clone(),
                        revision: link.revision.clone()
                    }),
                    Value::Null
                )),
                "{change}"
            );
            let row = record(s.conn(), &JudgmentSourceKind::Message, &message.id)
                .unwrap()
                .unwrap();
            assert_eq!(row.phase, JudgmentPhase::Suppressed, "{change}");
            assert!(row.attribution.is_none());
            assert!(row.recipients.is_empty());
            let unchanged = messages::get(s.conn(), &message.id).unwrap();
            assert!(unchanged.in_reply_to.is_none());
            assert!(unchanged.mentions.is_empty());
        }
    }

    #[test]
    fn message_attribution_is_independent_and_cross_crew_inspection_refuses() {
        let mut s = store();
        configured(&mut s);
        let original = ok(&mut s, |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: "context".into(),
                    ..Default::default()
                },
            )
        })
        .message;
        let message = ok(&mut s, |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: "follow-up".into(),
                    ..Default::default()
                },
            )
        })
        .message;
        let (r, input, _) = ok(&mut s, |tx| {
            dispatch(tx, &JudgmentSourceKind::Message, &message.id)
        })
        .unwrap();
        let candidate = input.links.iter().find(|l| l.id == original.id).unwrap();
        assert!(ok(&mut s, |tx| settle(
            tx,
            &JudgmentSourceKind::Message,
            &message.id,
            &r.decision_id,
            vec!["cap-a".into()],
            Some(JudgmentLink {
                kind: JudgmentObjectKind::from_key(&candidate.kind).unwrap(),
                id: candidate.id.clone(),
                revision: candidate.revision.clone()
            }),
            Value::Null
        )));
        let current = messages::get(s.conn(), &message.id).unwrap();
        assert_eq!(current.in_reply_to, None);
        assert!(current.mentions.is_empty());
        assert_eq!(current.text, message.text);
        assert!(
            inspect(
                s.conn(),
                &Sender::Session("cap-x".into()),
                JudgmentInspect {
                    crew_id: "crew-a".into(),
                    kind: JudgmentSourceKind::Message,
                    id: message.id.clone()
                }
            )
            .is_err()
        );
        let mut v = projection(s.conn(), "message", &message.id)
            .unwrap()
            .unwrap();
        v[RECORD_KEY]["version"] = serde_json::json!(2);
        s.conn()
            .execute(
                "UPDATE messages SET extra=?1 WHERE id=?2",
                rusqlite::params![v.to_string(), message.id],
            )
            .unwrap();
        assert!(record(s.conn(), &JudgmentSourceKind::Message, &message.id).is_err());
    }
}
