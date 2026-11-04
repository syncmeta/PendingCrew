//! Historical discovery does not grant authority. Human confirmation captures NEW facts.
use crate::{Result, Sender, Tx, admin_cases, judgments, work};
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::Connection;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
const ROOT: &str = "admin.legacy.index.v1";
const ROW: &str = "admin.legacy.row.v1:";
const RECEIPT: &str = "admin.legacy.receipt.v1:";
const SOURCE_IDS: &str = "admin.case.id.v1:";
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    #[serde(default)]
    discovered_count: i64,
    #[serde(default)]
    unverified_sources: Vec<String>,
    epoch: String,
    after: Option<String>,
    complete: bool,
    seq: i64,
    #[serde(default)]
    upper: i64,
    buckets: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    crew: String,
    id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    value: T,
    digest: String,
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid legacy administrative evidence".into()).into()
}
fn human(s: &Sender) -> Result<()> {
    if matches!(s, Sender::Human) {
        Ok(())
    } else {
        Err(crate::CollabError::Forbidden(crate::refusal::Refusal::new(
            crate::refusal::Code::ResourceForbidden,
        )))
    }
}
fn absent(id: &str) -> crate::CollabError {
    crate::CollabError::NotFound(
        crate::refusal::Refusal::new(crate::refusal::Code::ResourceNotFound).arg("id", id),
    )
}
fn read<T: DeserializeOwned + Serialize>(c: &Connection, key: &str) -> Result<Option<T>> {
    let Some(raw) = meta_get(c, key)? else {
        return Ok(None);
    };
    let e: Envelope<T> = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if e.version != 1 || e.digest != judgments::fingerprint(&(e.version, &e.value))? {
        return Err(invalid());
    }
    Ok(Some(e.value))
}
fn save<T: Serialize>(c: &Connection, key: &str, value: T) -> Result<()> {
    let digest = judgments::fingerprint(&(1u32, &value))?;
    meta_set(
        c,
        key,
        &serde_json::to_string(&Envelope {
            version: 1,
            value,
            digest,
        })
        .map_err(StoreError::from)?,
    )?;
    Ok(())
}
fn index(c: &Connection) -> Result<Index> {
    if let Some(i) = read(c, ROOT)? {
        return Ok(i);
    }
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key LIKE 'admin.legacy.%')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        return Err(invalid());
    }
    Ok(Index {
        epoch: crewd_store::judgment_state::epoch(c)?,
        ..Default::default()
    })
}
fn bucket_key(id: &str) -> Result<String> {
    Ok(format!(
        "admin.legacy.bucket.v1:{}",
        &judgments::fingerprint(&id)?[..2]
    ))
}
fn bucket(c: &Connection, i: &Index, k: &str) -> Result<BTreeMap<String, Link>> {
    let raw = meta_get(c, k)?;
    match (raw, i.buckets.get(k)) {
        (None, None) => Ok(BTreeMap::new()),
        (Some(raw), Some(hash)) if judgments::fingerprint(&raw)? == *hash => {
            serde_json::from_str(&raw).map_err(|e| StoreError::from(e).into())
        }
        _ => Err(invalid()),
    }
}
/// Per-announcement indexed evidence, protected by an independent bucket registry.
/// Lookup reads one hash bucket, never rescans all source/history records.
pub(crate) fn index_source(tx: &mut Tx<'_>, v: &AdminCaseView) -> Result<()> {
    let mut i = index(tx.conn())?;
    let k = bucket_key(&v.source.announcement_id)?;
    let mut b = bucket(tx.conn(), &i, &k)?;
    if let Some(old) = b.get(&v.source.announcement_id) {
        if old.id != v.id || old.crew != v.crew_id {
            return Err(invalid());
        }
    } else {
        b.insert(
            v.source.announcement_id.clone(),
            Link {
                crew: v.crew_id.clone(),
                id: v.id.clone(),
            },
        );
    }
    let raw = serde_json::to_string(&b).map_err(StoreError::from)?;
    i.buckets.insert(k.clone(), judgments::fingerprint(&raw)?);
    meta_set(tx.conn(), &k, &raw)?;
    save(tx.conn(), ROOT, i)?;
    put_link(
        tx,
        &source_key(&v.source.session_id, v.source.budget_epoch, &v.crew_id)?,
        Link {
            crew: v.crew_id.clone(),
            id: v.id.clone(),
        },
    )
}
fn lookup(c: &Connection, id: &str) -> Result<Option<Link>> {
    let i = index(c)?;
    Ok(bucket(c, &i, &bucket_key(id)?)?.get(id).cloned())
}
fn put_link(tx: &mut Tx<'_>, id: &str, link: Link) -> Result<()> {
    let mut i = index(tx.conn())?;
    let k = bucket_key(id)?;
    let mut b = bucket(tx.conn(), &i, &k)?;
    if let Some(old) = b.get(id)
        && (old.id != link.id || old.crew != link.crew)
    {
        return Err(invalid());
    }

    b.insert(id.into(), link);
    let raw = serde_json::to_string(&b).map_err(StoreError::from)?;
    i.buckets.insert(k.clone(), judgments::fingerprint(&raw)?);
    meta_set(tx.conn(), &k, &raw)?;
    save(tx.conn(), ROOT, i)
}
pub(crate) fn current_source(
    c: &Connection,
    session: &str,
    epoch: i64,
    crew: &str,
) -> Result<Option<String>> {
    let key = source_key(session, epoch, crew)?;
    let primary = meta_get(c, &key)?;
    if let Some(w) = lookup(c, &key)? {
        if primary.as_deref() != Some(w.id.as_str()) || w.crew != crew {
            return Err(invalid());
        }
    } else if index(c)?.complete && primary.is_some() {
        return Err(invalid());
    }
    Ok(primary)
}
fn linked(c: &Connection, id: &str) -> Result<Option<AdminCaseView>> {
    let i = index(c)?;
    let b = bucket(c, &i, &bucket_key(id)?)?;
    let Some(l) = b.get(id) else { return Ok(None) };
    let v = admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: l.crew.clone(),
            id: l.id.clone(),
        },
    )?;
    if v.source.announcement_id != id {
        return Err(invalid());
    }
    Ok(Some(v))
}
fn anchor(c: &Connection, id: &str) -> Result<(Message, String, bool)> {
    let m = crewd_store::messages::get(c, id)?.ok_or_else(|| absent(id))?;
    if m.category.as_deref() != Some("work_stop_escalation") {
        return Err(absent(id));
    }
    let ch = crate::channels::get(c, &m.channel_id)?;
    let home = crewd_store::crews::get(c, &ch.crew_id)?;
    let valid = m.sender_kind == SenderKind::System
        && m.sender_session_id.is_none()
        && ch.kind == ChannelKind::Human
        && home.is_some_and(|v| v.management)
        && m.mentions.iter().any(|v| v.kind == MentionKind::Human);
    let digest = judgments::fingerprint(&m)?;
    Ok((m, digest, valid))
}
fn live_view(c: &Connection, id: &str) -> Result<AdminCaseLegacyView> {
    let (m, digest, valid) = anchor(c, id)?;
    let link = linked(c, id)?;
    Ok(AdminCaseLegacyView {
        message_id: m.id,
        message_seq: m.seq,
        historical_text: if valid { m.text } else { String::new() },
        created_at: m.created_at,
        revision: digest,
        state: if link.is_some() {
            AdminCaseLegacyState::Linked
        } else if valid {
            AdminCaseLegacyState::Observed
        } else {
            AdminCaseLegacyState::Invalid
        },
        history_complete: false,
        linked_case_id: link.as_ref().map(|v| v.id.clone()),
        linked_crew_id: link.map(|v| v.crew_id),
        reason: if valid {
            AdminCaseLegacyReason::HistoricalHintOnly
        } else {
            AdminCaseLegacyReason::InvalidHistory
        },
    })
}
fn record_discovery(tx: &mut Tx<'_>, id: &str, seq: i64) -> Result<()> {
    let mut i = index(tx.conn())?;
    i.discovered_count = i.discovered_count.checked_add(1).ok_or_else(invalid)?;
    let ordinal = i.discovered_count;
    save(tx.conn(), ROOT, i)?;
    put_link(
        tx,
        &format!("discovery:{id}"),
        Link {
            crew: seq.to_string(),
            id: ordinal.to_string(),
        },
    )?;
    put_link(
        tx,
        &format!("ordinal:{ordinal}"),
        Link {
            crew: seq.to_string(),
            id: id.into(),
        },
    )
}
fn verified_source(c: &Connection, id: &str) -> Result<AdminCaseView> {
    let key = meta_get(c, &format!("{SOURCE_IDS}{id}"))?.ok_or_else(invalid)?;
    let raw = meta_get(c, &key)?.ok_or_else(invalid)?;
    let routing: serde_json::Value = serde_json::from_str(&raw).map_err(StoreError::from)?;
    let crew = routing["view"]["crewId"].as_str().ok_or_else(invalid)?;
    if key != admin_cases::key(crew, id)? {
        return Err(invalid());
    }
    let v = admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: crew.into(),
            id: id.into(),
        },
    )?;
    let budget_key = source_key(&v.source.session_id, v.source.budget_epoch, crew)?;
    if meta_get(c, &budget_key)?.as_deref() != Some(id) {
        return Err(invalid());
    }
    for key in [&v.source.announcement_id, &budget_key] {
        if let Some(w) = lookup(c, key)?
            && (w.id != id || w.crew != crew)
        {
            return Err(invalid());
        }
    }
    Ok(v)
}

fn retry_unverified_source(tx: &mut Tx<'_>) -> Result<()> {
    let i = index(tx.conn())?;
    let Some(id) = i.unverified_sources.first() else {
        return Ok(());
    };
    if let Ok(v) = verified_source(tx.conn(), id) {
        index_source(tx, &v)?;
        let mut latest = index(tx.conn())?;
        latest.unverified_sources.remove(0);
        save(tx.conn(), ROOT, latest)?;
    } else if i.unverified_sources.len() > 1 {
        let mut latest = i;
        latest.unverified_sources.rotate_left(1);
        save(tx.conn(), ROOT, latest)?;
    }
    Ok(())
}

fn backfill_sources(tx: &mut Tx<'_>, i: &Index) -> Result<Index> {
    let upper = meta_get(tx.conn(), "admin.case.sequence.v1")?
        .map(|s| s.parse::<u64>().map_err(|_| invalid()))
        .transpose()?
        .unwrap_or(0);
    let after = i.after.as_ref().map_or(Ok(0), |cursor| {
        cursor
            .strip_prefix(&format!("{SOURCE_IDS}AC"))
            .ok_or_else(invalid)?
            .parse::<u64>()
            .map_err(|_| invalid())
    })?;
    // Source IDs are allocated transactionally and never recycled. A missing
    // global alias must count as unverified rather than disappear from a page.
    let end = upper.min(after.saturating_add(64));
    if end < after
        || tx.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key>?1 AND key<?2)",
            [
                format!("{SOURCE_IDS}AC{upper:020}"),
                format!("{SOURCE_IDS}\u{10ffff}"),
            ],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid());
    }
    for ordinal in (after..end).map(|n| n + 1) {
        let id = format!("AC{ordinal:020}");
        match verified_source(tx.conn(), &id) {
            Ok(v) => {
                index_source(tx, &v)?;
                let mut latest = index(tx.conn())?;
                if let Some(position) = latest.unverified_sources.iter().position(|s| s == &id) {
                    latest.unverified_sources.remove(position);
                    save(tx.conn(), ROOT, latest)?;
                }
            }
            Err(_) => {
                let mut latest = index(tx.conn())?;
                if !latest.unverified_sources.contains(&id) {
                    latest.unverified_sources.push(id.clone());
                    save(tx.conn(), ROOT, latest)?;
                }
                save(
                    tx.conn(),
                    &format!("admin.legacy.source_error.v1:{id}"),
                    "invalid_source",
                )?;
            }
        }
    }
    let mut latest = index(tx.conn())?;
    latest.complete = end == upper;
    latest.after = (!latest.complete).then(|| format!("{SOURCE_IDS}AC{end:020}"));
    save(tx.conn(), ROOT, &latest)?;
    Ok(latest)
}
/// Bounded source-index backfill first; then at most 64 original message rows per call.
/// Cursor advances with discoveries. New messages are picked up on subsequent calls.
pub fn tick(tx: &mut Tx<'_>) -> Result<()> {
    let mut i = index(tx.conn())?;
    let epoch = crewd_store::judgment_state::epoch(tx.conn())?;
    if i.epoch != epoch {
        i.epoch = epoch;
        i.complete = false;
        i.after = None;
        i.seq = 0;
        i.upper = 0;
        save(tx.conn(), ROOT, &i)?;
    }
    retry_unverified_source(tx)?;
    if !i.complete {
        i = backfill_sources(tx, &i)?;
        if !i.complete {
            return Ok(());
        }
    }
    if i.seq >= i.upper {
        i.upper = tx
            .conn()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM messages", [], |r| {
                r.get(0)
            })?;
    }
    let ids = {
        let mut q=tx.conn().prepare("SELECT seq,id FROM messages WHERE category='work_stop_escalation' AND seq>?1 AND seq<=?2 ORDER BY seq LIMIT 64")?;
        q.query_map([i.seq, i.upper], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let complete = ids.len() < 64;
    for (seq, id) in ids {
        let category: Option<String> =
            tx.conn()
                .query_row("SELECT category FROM messages WHERE id=?1", [&id], |r| {
                    r.get(0)
                })?;
        if category.as_deref() == Some("work_stop_escalation") {
            let key = format!("{ROW}{seq:020}");
            if lookup(tx.conn(), &format!("discovery:{id}"))?.is_some() {
                i.seq = seq;
                continue;
            }
            if read::<AdminCaseLegacyView>(tx.conn(), &key).is_err() {
                i.seq = seq;
                continue;
            }
            match live_view(tx.conn(), &id) {
                Ok(v) => save(tx.conn(), &key, v)?,
                Err(_) => {
                    let (text, at, revision) = anchor(tx.conn(), &id)
                        .ok()
                        .filter(|(_, _, valid)| *valid)
                        .map(|(m, digest, _)| (m.text, m.created_at, digest))
                        .unwrap_or_else(|| (String::new(), String::new(), "invalid".into()));
                    save(
                        tx.conn(),
                        &key,
                        AdminCaseLegacyView {
                            message_id: id.clone(),
                            message_seq: seq,
                            historical_text: text,
                            created_at: at,
                            revision,
                            state: AdminCaseLegacyState::Invalid,
                            history_complete: false,
                            linked_case_id: None,
                            linked_crew_id: None,
                            reason: AdminCaseLegacyReason::InvalidEvidence,
                        },
                    )?;
                }
            }
            record_discovery(tx, &id, seq)?;
        }
        i.seq = seq;
    }
    if complete {
        i.seq = i.upper;
    }
    let mut latest = index(tx.conn())?;
    latest.seq = i.seq;
    latest.upper = i.upper;
    save(tx.conn(), ROOT, latest)
}
pub fn get(c: &Connection, sender: &Sender, id: &str) -> Result<AdminCaseLegacyView> {
    human(sender)?;
    let mut view = live_view(c, id)?;
    let discovery = lookup(c, &format!("discovery:{id}"))?;
    let saved = read::<AdminCaseLegacyView>(c, &format!("{ROW}{:020}", view.message_seq))?;
    if (discovery.is_some() && saved.is_none())
        || (discovery.is_none() && (saved.is_some() || view.message_seq <= index(c)?.seq))
    {
        return Err(invalid());
    }
    if let Some(w) = discovery {
        let ordinal = lookup(c, &format!("ordinal:{}", w.id))?.ok_or_else(invalid)?;
        if w.crew != view.message_seq.to_string() || ordinal.id != id || ordinal.crew != w.crew {
            return Err(invalid());
        }
    }
    if let Some(saved) = saved {
        if saved.message_id != id
            || (saved.revision != "invalid" && saved.revision != view.revision)
        {
            return Err(invalid());
        }
        if saved.linked_case_id.is_some() {
            view.linked_case_id = saved.linked_case_id;
            view.linked_crew_id = saved.linked_crew_id;
            view.state = AdminCaseLegacyState::Linked;
        }
    }
    if let Some(link) = lookup(c, &format!("recovery:{id}"))? {
        let r = receipt(c, sender, &link.id)?;
        if r.message_id != id || r.crew_id != link.crew {
            return Err(invalid());
        }
        view.state = AdminCaseLegacyState::Linked;
        view.linked_case_id = Some(r.case_id);
        view.linked_crew_id = Some(r.crew_id);
        view.reason = AdminCaseLegacyReason::HumanRevalidated;
    }
    Ok(view)
}
pub fn list(
    c: &Connection,
    sender: &Sender,
    after: Option<String>,
    limit: u32,
) -> Result<AdminCaseLegacyList> {
    human(sender)?;
    if !(1..=100).contains(&limit) {
        return Err(crate::resource_acl::invalid("limit"));
    }
    let epoch = judgments::fingerprint(&crewd_store::judgment_state::epoch(c)?)?;
    let seq = match after {
        None => 0,
        Some(s) => s
            .strip_prefix(&format!("{epoch}:"))
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|n| *n >= 0)
            .ok_or_else(|| crate::resource_acl::invalid("after"))?,
    };
    let i = index(c)?;
    if seq > i.discovered_count {
        return Err(crate::resource_acl::invalid("after"));
    }
    let last = i.discovered_count.min(seq + i64::from(limit));
    let more = last < i.discovered_count;
    let mut items = Vec::new();
    for ordinal in seq + 1..=last {
        let w = lookup(c, &format!("ordinal:{ordinal}"))?.ok_or_else(invalid)?;
        let message_seq = w.crew.parse::<i64>().map_err(|_| invalid())?;
        match get(c, sender, &w.id) {
            Ok(v) => items.push(v),
            Err(_) => {
                let saved = read::<AdminCaseLegacyView>(c, &format!("{ROW}{message_seq:020}"))
                    .ok()
                    .flatten()
                    .filter(|v| v.message_id == w.id && v.message_seq == message_seq);
                let (historical_text, created_at) = if let Some(v) = saved {
                    (v.historical_text, v.created_at)
                } else {
                    anchor(c, &w.id)
                        .ok()
                        .filter(|(m, _, valid)| *valid && m.seq == message_seq)
                        .map(|(m, _, _)| (m.text, m.created_at))
                        .unwrap_or_default()
                };
                items.push(AdminCaseLegacyView {
                    message_id: w.id,
                    message_seq,
                    historical_text,
                    created_at,
                    revision: "invalid".into(),
                    state: AdminCaseLegacyState::Invalid,
                    history_complete: false,
                    linked_case_id: None,
                    linked_crew_id: None,
                    reason: AdminCaseLegacyReason::InvalidEvidence,
                });
            }
        }
    }
    let i = index(c)?;
    let scan = AdminCaseLegacyScan {
        phase: if !i.complete || !i.unverified_sources.is_empty() {
            AdminCaseLegacyScanPhase::Indexing
        } else if i.seq < i.upper {
            AdminCaseLegacyScanPhase::Scanning
        } else {
            AdminCaseLegacyScanPhase::Current
        },
        through_seq: i.seq,
        captured_upper: i.upper,
        unverified_sources: i.unverified_sources.len() as u32,
    };
    Ok(AdminCaseLegacyList {
        items,
        next_after: more.then(|| format!("{epoch}:{last}")),
        scan,
    })
}
fn source_key(session: &str, epoch: i64, crew: &str) -> Result<String> {
    Ok(format!(
        "admin.case.source.v1:{}",
        judgments::fingerprint(&(session, epoch, crew))?
    ))
}
pub fn preview(
    c: &Connection,
    sender: &Sender,
    p: &AdminCaseLegacyPreviewParams,
    now: i64,
) -> Result<AdminCaseLegacyPreview> {
    human(sender)?;
    let h = get(c, sender, &p.message_id)?;
    let mut blockers = Vec::new();
    if h.linked_case_id.is_some() {
        blockers.push(AdminCaseLegacyBlocker::AlreadyLinked)
    }
    if h.state == AdminCaseLegacyState::Invalid {
        blockers.push(AdminCaseLegacyBlocker::InvalidHistory)
    }
    let idx = index(c)?;
    if !idx.complete
        || !idx.unverified_sources.is_empty()
        || idx.epoch != crewd_store::judgment_state::epoch(c)?
    {
        blockers.push(AdminCaseLegacyBlocker::IndexIncomplete)
    }
    let session = crewd_store::sessions::get(c, &p.session_id)?;
    if session.as_ref().is_none_or(|s| {
        s.crew_id != p.crew_id || s.desired != Desired::Running || s.resume_failed_at.is_some()
    }) {
        blockers.push(AdminCaseLegacyBlocker::SourceUnavailable)
    }
    let crew = crewd_store::crews::get(c, &p.crew_id)?;
    if crew.is_none() || crate::crews::is_archived(c, &p.crew_id)? {
        blockers.push(AdminCaseLegacyBlocker::CrewUnavailable)
    }
    if crate::wakes::automatic_work_paused(c, &p.session_id)? {
        blockers.push(AdminCaseLegacyBlocker::Stopped)
    }
    let raw = if session.as_ref().is_some_and(|s| s.crew_id == p.crew_id) {
        crate::stop_checks::targets(c, &p.session_id, now)?
    } else {
        Vec::new()
    };
    let budget = crewd_store::work_stop_checks::get(c, &p.session_id)?;
    match &budget {
        None => blockers.push(AdminCaseLegacyBlocker::MissingBudget),
        Some(b) => {
            if !b.streak.escalated {
                blockers.push(AdminCaseLegacyBlocker::NotEscalated)
            }
            if b.streak.last.len() != raw.len()
                || raw
                    .iter()
                    .map(|t| Ok((work::number(&t.id)?, t.kind)))
                    .collect::<Result<Vec<_>>>()?
                    .iter()
                    .any(|(n, k)| {
                        !b.streak
                            .last
                            .iter()
                            .any(|v| i64::from(v.item.0) == *n && v.kind == *k)
                    })
            {
                blockers.push(AdminCaseLegacyBlocker::BudgetMismatch)
            }
        }
    }
    let mut targets = Vec::new();
    for t in &raw {
        let w = work::get(c, &t.id)?;
        targets.push(AdminCaseTarget {
            id: t.id.clone(),
            crew_id: w.crew_id.clone(),
            revision: t.revision.clone(),
            kind: match t.kind {
                crewd_work::StopKind::Unanswered => AdminCaseTargetKind::Unanswered,
                crewd_work::StopKind::Overdue => AdminCaseTargetKind::Overdue,
                crewd_work::StopKind::ReadyUnscheduled => AdminCaseTargetKind::ReadyUnscheduled,
            },
            scope: if w.crew_id == p.crew_id {
                AdminCaseScope::InScope
            } else {
                AdminCaseScope::OutOfScope
            },
        });
    }
    if targets.is_empty() {
        blockers.push(AdminCaseLegacyBlocker::NoTargets)
    } else if !targets.iter().any(|t| t.scope == AdminCaseScope::InScope) {
        blockers.push(AdminCaseLegacyBlocker::NoInScopeTargets)
    }
    let existing = if let Some(b) = &budget {
        current_source(c, &p.session_id, b.epoch, &p.crew_id)?
    } else {
        None
    };
    if let Some(id) = &existing {
        admin_cases::source_get(
            c,
            &AdminCaseGetParams {
                crew_id: p.crew_id.clone(),
                id: id.clone(),
            },
        )?;
    }
    let restore = crewd_store::judgment_state::epoch(c)?;
    let revision = judgments::fingerprint(&(
        &h.revision,
        p,
        &targets,
        budget.as_ref().map(|b| (b.epoch, &b.streak)),
        &session,
        &crew,
        judgments::configuration(c)?,
        &restore,
        judgments::stop_epoch(c, &p.session_id)?,
        crate::wakes::human_stop_floor(c, &p.session_id)?,
        &existing,
        &blockers,
    ))?;
    Ok(AdminCaseLegacyPreview {
        message_id: p.message_id.clone(),
        crew_id: p.crew_id.clone(),
        session_id: p.session_id.clone(),
        revision,
        can_confirm: blockers.is_empty(),
        blockers,
        targets,
        budget_epoch: budget.map(|b| b.epoch),
        restore_epoch: restore,
        existing_case_id: existing,
        observed_at: now,
    })
}
pub fn receipt(c: &Connection, sender: &Sender, id: &str) -> Result<AdminCaseLegacyReceipt> {
    human(sender)?;
    uuid::Uuid::parse_str(id).map_err(|_| crate::resource_acl::invalid("requestId"))?;
    let primary = read::<AdminCaseLegacyReceipt>(c, &format!("{RECEIPT}{id}"))?;
    let w = lookup(c, &format!("receipt:{id}"))?;
    match (primary, w) {
        (None, None) => Err(absent(id)),
        (Some(r), Some(w)) if r.request_id == id && r.case_id == w.id && r.crew_id == w.crew => {
            Ok(r)
        }
        _ => Err(invalid()),
    }
}
pub fn confirm(
    tx: &mut Tx<'_>,
    sender: &Sender,
    p: AdminCaseLegacyConfirmParams,
) -> Result<AdminCaseLegacyReceipt> {
    human(sender)?;
    uuid::Uuid::parse_str(&p.request_id).map_err(|_| crate::resource_acl::invalid("requestId"))?;
    if p.note.trim().is_empty() || p.note.len() > 2000 {
        return Err(crate::resource_acl::invalid("note"));
    }
    if read::<AdminCaseLegacyReceipt>(tx.conn(), &format!("{RECEIPT}{}", p.request_id))?.is_some()
        || lookup(tx.conn(), &format!("receipt:{}", p.request_id))?.is_some()
    {
        let old = receipt(tx.conn(), sender, &p.request_id)?;
        if old.message_id != p.message_id
            || old.crew_id != p.crew_id
            || old.session_id != p.session_id
            || old.note != p.note.trim()
            || old.preview_revision != p.expected_preview_revision
        {
            return Err(invalid());
        }
        return Ok(old);
    }
    let preview = preview(
        tx.conn(),
        sender,
        &AdminCaseLegacyPreviewParams {
            message_id: p.message_id.clone(),
            crew_id: p.crew_id.clone(),
            session_id: p.session_id.clone(),
        },
        tx.now_ms(),
    )?;
    if !preview.can_confirm || preview.revision != p.expected_preview_revision {
        return Err(crate::resource_acl::invalid("expectedPreviewRevision"));
    }
    let h = get(tx.conn(), sender, &p.message_id)?;
    if h.linked_case_id.is_some() {
        return Err(crate::resource_acl::invalid("messageId"));
    }
    let raw = crate::stop_checks::targets(tx.conn(), &p.session_id, tx.now_ms())?;
    let v = admin_cases::record_revalidated_source(
        tx,
        &p.session_id,
        &raw,
        &p.message_id,
        AdminCaseProvenance::HumanRevalidation {
            request_id: p.request_id.clone(),
            legacy_message_id: p.message_id.clone(),
            announcement_digest: h.revision.clone(),
            preview_revision: preview.revision.clone(),
            note: p.note.trim().into(),
            revalidated_at: tx.now_ms(),
        },
    )?;
    let r = AdminCaseLegacyReceipt {
        request_id: p.request_id,
        message_id: p.message_id,
        crew_id: p.crew_id,
        session_id: p.session_id,
        case_id: v.id.clone(),
        created: preview.existing_case_id.is_none(),
        confirmed_at: tx.now_ms(),
        note: p.note.trim().into(),
        preview_revision: preview.revision,
    };
    save(tx.conn(), &format!("{RECEIPT}{}", r.request_id), &r)?;
    put_link(
        tx,
        &format!("receipt:{}", r.request_id),
        Link {
            crew: r.crew_id.clone(),
            id: r.case_id.clone(),
        },
    )?;
    put_link(
        tx,
        &format!("recovery:{}", r.message_id),
        Link {
            crew: r.crew_id.clone(),
            id: r.request_id.clone(),
        },
    )?;
    let mut h = h;
    h.state = AdminCaseLegacyState::Linked;
    h.linked_case_id = Some(v.id);
    h.linked_crew_id = Some(v.crew_id);
    h.reason = AdminCaseLegacyReason::HumanRevalidated;
    if lookup(tx.conn(), &format!("discovery:{}", h.message_id))?.is_none() {
        record_discovery(tx, &h.message_id, h.message_seq)?;
    }
    save(tx.conn(), &format!("{ROW}{:020}", h.message_seq), h)?;
    Ok(r)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok, store};
    fn unscanned_fixture(s: &mut crewd_store::Store) -> String {
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='w-a1'", [])
            .unwrap();
        let w = ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    owner: Some("w-a1".into()),
                    title: "current synthetic work".into(),
                    ..Default::default()
                },
            )
        });
        let streak = crewd_work::StopStreak {
            last: vec![crewd_work::StopViolation {
                item: w.id.parse().unwrap(),
                kind: crewd_work::StopKind::ReadyUnscheduled,
            }],
            escalated: true,
            unimproved: 3,
            rewakes: 3,
        };
        s.conn()
            .execute(
                "INSERT INTO work_stop_checks(session_id,epoch,state) VALUES('w-a1',0,?1)",
                [serde_json::to_string(&streak).unwrap()],
            )
            .unwrap();
        ok(s, |tx| {
            crate::messages::announce_to_human(
                tx,
                "chief",
                Sender::System,
                "Historical text is only a hint; not parsed as an identity".into(),
                "work_stop_escalation",
            )
        })
        .message
        .id
    }
    fn fixture(s: &mut crewd_store::Store) -> String {
        let id = unscanned_fixture(s);
        ok(s, tick);
        id
    }
    fn params(s: &crewd_store::Store, id: &str) -> AdminCaseLegacyConfirmParams {
        let v = preview(
            s.conn(),
            &Sender::Human,
            &AdminCaseLegacyPreviewParams {
                message_id: id.into(),
                crew_id: "crew-a".into(),
                session_id: "w-a1".into(),
            },
            NOW,
        )
        .unwrap();
        assert!(v.can_confirm, "{:?}", v.blockers);
        AdminCaseLegacyConfirmParams {
            message_id: id.into(),
            crew_id: "crew-a".into(),
            session_id: "w-a1".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_preview_revision: v.revision,
            note: "Human verified current full evidence".into(),
        }
    }
    #[test]
    fn epoch_zero_new_human_source_rollback_idempotency_and_provenance() {
        let mut s = store();
        let id = fixture(&mut s);
        let p = params(&s, &id);
        let before = crewd_store::work_stop_checks::get(s.conn(), "w-a1").unwrap();
        let result: Result<((), Vec<crate::Change>)> = crate::run(&mut s, NOW, |tx| {
            confirm(tx, &Sender::Human, p.clone())?;
            Err(invalid())
        });
        assert!(result.is_err());
        assert!(matches!(
            receipt(s.conn(), &Sender::Human, &p.request_id),
            Err(crate::CollabError::NotFound(_))
        ));
        let r = ok(&mut s, |tx| confirm(tx, &Sender::Human, p.clone()));
        assert_eq!(r, ok(&mut s, |tx| confirm(tx, &Sender::Human, p.clone())));
        assert_eq!(
            before,
            crewd_store::work_stop_checks::get(s.conn(), "w-a1").unwrap()
        );
        let v = admin_cases::source_get(
            s.conn(),
            &AdminCaseGetParams {
                crew_id: "crew-a".into(),
                id: r.case_id,
            },
        )
        .unwrap();
        assert_eq!(v.source.budget_epoch, 0);
        assert!(v.source.provenance.is_some());
        assert_eq!(v.source.announcement_id, id);
        let changes = s.conn().total_changes();
        get(s.conn(), &Sender::Human, &id).unwrap();
        list(s.conn(), &Sender::Human, None, 20).unwrap();
        receipt(s.conn(), &Sender::Human, &p.request_id).unwrap();
        assert_eq!(changes, s.conn().total_changes());
    }
    #[test]
    fn damaged_discovery_visible_and_missing_receipt_index_fails_closed() {
        let mut s = store();
        let id = fixture(&mut s);
        let p = params(&s, &id);
        let r = ok(&mut s, |tx| confirm(tx, &Sender::Human, p));
        let v = get(s.conn(), &Sender::Human, &id).unwrap();
        meta_set(s.conn(), &format!("{ROW}{:020}", v.message_seq), "broken").unwrap();
        let page = list(s.conn(), &Sender::Human, None, 20).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].state, AdminCaseLegacyState::Invalid);
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("{RECEIPT}{}", r.request_id)],
            )
            .unwrap();
        assert!(matches!(
            receipt(s.conn(), &Sender::Human, &r.request_id),
            Err(crate::CollabError::Store(_))
        ));
    }
    #[test]
    fn modern_announcement_and_source_index_corruption_never_become_legacy() {
        let mut s = store();
        let v = crate::admin_case_execution::tests::adopted(&mut s);
        ok(&mut s, tick);
        assert_eq!(
            get(s.conn(), &Sender::Human, &v.source.announcement_id)
                .unwrap()
                .linked_case_id,
            Some(v.id.clone())
        );
        let b = bucket_key(&v.source.announcement_id).unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [b])
            .unwrap();
        assert!(get(s.conn(), &Sender::Human, &v.source.announcement_id).is_err());
        assert!(
            serde_json::to_value(&v.source)
                .unwrap()
                .get("provenance")
                .is_none()
        );
    }
    #[test]
    fn epoch_zero_native_stop_cannot_manufacture_a_source() {
        let mut s = store();
        let id = fixture(&mut s);
        let raw = crate::stop_checks::targets(s.conn(), "w-a1", NOW).unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| admin_cases::record_stop_escalation(
                tx, "w-a1", &raw, &id
            ))
            .is_err()
        );
        assert!(linked(s.conn(), &id).unwrap().is_none());
        assert!(
            admin_cases::source_page(s.conn(), None)
                .unwrap()
                .0
                .is_empty()
        );
    }
    #[test]
    fn confirmation_before_scan_is_discoverable_immediately_and_after_tick() {
        let mut s = store();
        ok(&mut s, tick);
        let id = unscanned_fixture(&mut s);
        let p = params(&s, &id);
        assert!(
            list(s.conn(), &Sender::Human, None, 20)
                .unwrap()
                .items
                .is_empty()
        );
        let r = ok(&mut s, |tx| confirm(tx, &Sender::Human, p));
        for scan in [false, true] {
            if scan {
                ok(&mut s, tick);
            }
            let v = get(s.conn(), &Sender::Human, &id).unwrap();
            let items = list(s.conn(), &Sender::Human, None, 20).unwrap().items;
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].message_id, id);
            assert_eq!(v.linked_case_id.as_deref(), Some(r.case_id.as_str()));
            assert_eq!(v.reason, AdminCaseLegacyReason::HumanRevalidated);
        }
    }
    #[test]
    fn list_preserves_saved_history_on_changed_or_deleted_message() {
        let mut s = store();
        let id = fixture(&mut s);
        let original = get(s.conn(), &Sender::Human, &id).unwrap();
        s.conn()
            .execute(
                "UPDATE messages SET text='replacement',category=NULL WHERE id=?1",
                [&id],
            )
            .unwrap();
        for deleted in [false, true] {
            if deleted {
                s.conn()
                    .execute("DELETE FROM messages WHERE id=?1", [&id])
                    .unwrap();
            }
            let v = list(s.conn(), &Sender::Human, None, 20)
                .unwrap()
                .items
                .remove(0);
            assert_eq!(v.state, AdminCaseLegacyState::Invalid);
            assert_eq!(v.historical_text, original.historical_text);
            assert_eq!(v.created_at, original.created_at);
        }
    }
    #[test]
    fn direct_legacy_get_does_not_reveal_ordinary_messages() {
        let mut s = store();
        ok(&mut s, tick);
        let id = unscanned_fixture(&mut s);
        s.conn()
            .execute(
                "UPDATE messages SET text='ordinary private text',category=NULL WHERE id=?1",
                [&id],
            )
            .unwrap();
        assert!(matches!(
            get(s.conn(), &Sender::Human, &id),
            Err(crate::CollabError::NotFound(_))
        ));
        s.conn().execute(
            "UPDATE messages SET sender_kind='human',category='work_stop_escalation' WHERE id=?1",
            [&id],
        ).unwrap();
        let v = get(s.conn(), &Sender::Human, &id).unwrap();
        assert_eq!(v.state, AdminCaseLegacyState::Invalid);
        assert!(v.historical_text.is_empty());
    }
    #[test]
    fn old_typed_source_with_missing_global_alias_blocks_legacy_until_repaired() {
        let mut s = store();
        let v = crate::admin_case_execution::tests::adopted(&mut s);
        // Before the new historical index existed, only native case metadata
        // could distinguish this message from a historical announcement.
        s.conn()
            .execute("DELETE FROM meta WHERE key LIKE 'admin.legacy.%'", [])
            .unwrap();
        let global = format!("{SOURCE_IDS}{}", v.id);
        let key = meta_get(s.conn(), &global).unwrap().unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [&global])
            .unwrap();
        ok(&mut s, tick);
        let p = AdminCaseLegacyPreviewParams {
            message_id: v.source.announcement_id,
            crew_id: v.crew_id,
            session_id: "w-a2".into(),
        };
        let before = preview(s.conn(), &Sender::Human, &p, NOW).unwrap();
        assert!(
            before
                .blockers
                .contains(&AdminCaseLegacyBlocker::IndexIncomplete)
        );
        assert!(!before.can_confirm);
        meta_set(s.conn(), &global, &key).unwrap();
        ok(&mut s, tick);
        let after = preview(s.conn(), &Sender::Human, &p, NOW).unwrap();
        assert!(
            !after
                .blockers
                .contains(&AdminCaseLegacyBlocker::IndexIncomplete)
        );
        assert!(
            after
                .blockers
                .contains(&AdminCaseLegacyBlocker::AlreadyLinked)
        );
        assert!(!after.can_confirm);
    }
    #[test]
    fn unverified_sources_rotate_and_clear_only_after_full_repair() {
        let mut s = store();
        let v = crate::admin_case_execution::tests::adopted(&mut s);
        let key = meta_get(s.conn(), &format!("admin.case.id.v1:{}", v.id))
            .unwrap()
            .unwrap();
        let raw = meta_get(s.conn(), &key).unwrap().unwrap();
        meta_set(s.conn(), &key, "corrupt case").unwrap();
        ok(&mut s, tick);
        assert_eq!(index(s.conn()).unwrap().unverified_sources, [v.id.clone()]);
        let missing = "AC00000000000000000099".to_string();
        let mut i = index(s.conn()).unwrap();
        i.unverified_sources.insert(0, missing.clone());
        save(s.conn(), ROOT, i).unwrap();
        let budget = source_key(&v.source.session_id, v.source.budget_epoch, &v.crew_id).unwrap();
        meta_set(s.conn(), &key, &raw).unwrap();
        meta_set(s.conn(), &budget, "wrong case").unwrap();
        ok(&mut s, tick);
        assert_eq!(
            index(s.conn()).unwrap().unverified_sources,
            [v.id.clone(), missing.clone()]
        );
        ok(&mut s, tick);
        assert_eq!(
            index(s.conn()).unwrap().unverified_sources,
            [missing.clone(), v.id.clone()]
        );
        meta_set(s.conn(), &budget, &v.id).unwrap();
        ok(&mut s, tick);
        ok(&mut s, tick);
        assert_eq!(index(s.conn()).unwrap().unverified_sources, [missing]);
        assert!(
            meta_get(s.conn(), &format!("admin.legacy.source_error.v1:{}", v.id))
                .unwrap()
                .is_some()
        );
    }
}
