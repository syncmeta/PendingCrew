//! Explicit case custody: notification delivery is not acceptance or resolution.
use crate::{Result, Sender, Tx};
use crewd_proto::*;
use rusqlite::Connection;
#[derive(Clone, Debug)]
pub enum Operation {
    Request {
        request_id: String,
        reason: String,
    },
    Get {
        request_id: String,
    },
    Accept {
        request_id: String,
        expected_revision: String,
        note: String,
    },
    List {
        after: Option<String>,
        limit: u32,
    },
}
#[derive(Clone)]
pub struct ReadPermit {
    source: String,
    admin: String,
    captain: String,
    case: String,
    crew: String,
}
impl ReadPermit {
    pub fn source_session_id(&self) -> &str {
        &self.source
    }
    pub fn admin_session_id(&self) -> &str {
        &self.admin
    }
    pub fn captain_session_id(&self) -> &str {
        &self.captain
    }
    pub fn case_id(&self) -> &str {
        &self.case
    }
    pub fn crew_id(&self) -> &str {
        &self.crew
    }
}
pub struct RequestPermit {
    read: ReadPermit,
    operation: Operation,
    before_seq: u64,
}
pub struct AcceptPermit {
    read: ReadPermit,
    operation: Operation,
    before_seq: u64,
}
macro_rules! getters {
    ($t:ty) => {
        impl $t {
            pub fn source_session_id(&self) -> &str {
                self.read.source_session_id()
            }
            pub fn admin_session_id(&self) -> &str {
                self.read.admin_session_id()
            }
            pub fn captain_session_id(&self) -> &str {
                self.read.captain_session_id()
            }
            pub fn case_id(&self) -> &str {
                self.read.case_id()
            }
            pub fn crew_id(&self) -> &str {
                self.read.crew_id()
            }
        }
    };
}
getters!(RequestPermit);
getters!(AcceptPermit);

use crate::{admin_case_execution as execution, admin_cases, judgments, wakes};
use AdminCaseHandoffPhase as Phase;
use AdminCaseHandoffReason as Reason;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::params;
use serde::{Deserialize, Serialize};
const PREFIX: &str = "admin.case.handoff.v1:";
const GLOBAL: &str = "admin.case.handoff.sequence.v1:";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    sequence: u64,
    attempt: String,
    authority: String,
    checkpoint: String,
    captain_context: String,
    view: AdminCaseHandoffView,
    accepted_revision: Option<String>,
    checksum: String,
}
fn invalid() -> crate::CollabError {
    crate::resource_acl::invalid("admin_case_handoff")
}
fn key(id: &str) -> String {
    format!("{PREFIX}{id}")
}
fn active_key(id: &str) -> String {
    format!("admin.case.handoff.active.v1:{id}")
}
fn relinquished_key(id: &str) -> String {
    format!("admin.case.handoff.relinquished.v1:{id}")
}
fn index_prefix(captain: &str) -> Result<String> {
    Ok(format!(
        "admin.case.handoff.captain.v1:{}:",
        judgments::fingerprint(&captain)?
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipientWitness {
    version: u32,
    recipient: String,
    count: u64,
    digest: String,
    checksum: String,
}
fn recipient_keys(recipient: &str) -> Result<(String, String)> {
    let id = judgments::fingerprint(&recipient)?;
    Ok((
        format!("admin.case.handoff.recipientWitness.v1:{id}"),
        format!("admin.case.handoff.recipientPresent.v1:{id}"),
    ))
}
fn index_digest(c: &Connection, recipient: &str) -> Result<(u64, String)> {
    let prefix = index_prefix(recipient)?;
    let upper = format!("{prefix}\u{10ffff}");
    let mut after = prefix.clone();
    let mut count = 0u64;
    let mut digest = judgments::fingerprint(&("handoff-recipient-index-v1", recipient))?;
    loop {
        let mut stmt =
            c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT 64")?;
        let rows = stmt
            .query_map(params![after, upper], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok((count, digest));
        }
        for (key, id) in rows {
            count = count.checked_add(1).ok_or_else(invalid)?;
            digest = judgments::fingerprint(&(&digest, &key, &id))?;
            after = key;
        }
    }
}
fn witness_checksum(w: &RecipientWitness) -> Result<String> {
    judgments::fingerprint(&(w.version, &w.recipient, w.count, &w.digest))
}
fn verify_recipient(c: &Connection, recipient: &str) -> Result<()> {
    let (key, marker) = recipient_keys(recipient)?;
    let raw = meta_get(c, &key)?;
    let present = meta_get(c, &marker)?;
    let (count, digest) = index_digest(c, recipient)?;
    match (raw, present) {
        (None, None) if count == 0 => Ok(()),
        (Some(raw), Some(marker)) if marker == "1" => {
            let w: RecipientWitness = serde_json::from_str(&raw).map_err(StoreError::from)?;
            if w.version != 1
                || w.recipient != recipient
                || w.count == 0
                || w.count != count
                || w.digest != digest
                || w.checksum != witness_checksum(&w)?
            {
                return Err(invalid());
            }
            Ok(())
        }
        _ => Err(invalid()),
    }
}
fn save_recipient(c: &Connection, recipient: &str) -> Result<()> {
    let (key, marker) = recipient_keys(recipient)?;
    let (count, digest) = index_digest(c, recipient)?;
    let mut w = RecipientWitness {
        version: 1,
        recipient: recipient.into(),
        count,
        digest,
        checksum: String::new(),
    };
    w.checksum = witness_checksum(&w)?;
    meta_set(
        c,
        &key,
        &serde_json::to_string(&w).map_err(StoreError::from)?,
    )?;
    meta_set(c, &marker, "1")?;
    Ok(())
}
fn digest(r: &Saved) -> Result<String> {
    judgments::fingerprint(&(
        r.version,
        r.sequence,
        &r.attempt,
        &r.authority,
        &r.checkpoint,
        &r.captain_context,
        &r.view,
        &r.accepted_revision,
    ))
}
fn save(c: &Connection, r: &mut Saved) -> Result<()> {
    r.checksum = digest(r)?;
    meta_set(
        c,
        &key(&r.view.request_id),
        &serde_json::to_string(r).map_err(StoreError::from)?,
    )?;
    Ok(())
}
fn load(c: &Connection, id: &str) -> Result<Option<Saved>> {
    let Some(raw) = meta_get(c, &key(id))? else {
        return Ok(None);
    };
    let r: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if r.version != 1
        || r.sequence == 0
        || r.view.request_id != id
        || r.checksum != digest(&r)?
        || !matches!(r.view.phase, Phase::Waiting | Phase::Accepted)
        || (r.view.phase == Phase::Accepted
            && (r.view.accepted_at.is_none()
                || r.view.acceptance_note.is_none()
                || r.view.delivered_at.is_none()
                || r.accepted_revision.is_none()))
    {
        return Err(StoreError::Invalid("corrupt administrative handoff".into()).into());
    }
    if meta_get(c, &format!("{GLOBAL}{:020}", r.sequence))?.as_deref() != Some(id)
        || meta_get(
            c,
            &format!(
                "{}{:020}",
                index_prefix(&r.view.captain_session_id)?,
                r.sequence
            ),
        )?
        .as_deref()
            != Some(id)
        || (r.view.phase == Phase::Accepted
            && meta_get(c, &relinquished_key(&r.view.case_id))?.as_deref() != Some(id))
    {
        return Err(invalid());
    }
    Ok(Some(r))
}
fn session(by: &Sender) -> Result<&str> {
    match by {
        Sender::Session(id) => Ok(id),
        _ => Err(invalid()),
    }
}
fn actor(c: &Connection, by: &Sender) -> Result<Session> {
    let s = crate::sessions::get(c, session(by)?)?;
    crate::crews::refuse_archived(c, &s.crew_id)?;
    Ok(s)
}
fn normalized(value: &str) -> Result<String> {
    let s = value.trim();
    if s.is_empty() || s.chars().count() > 2000 {
        return Err(invalid());
    }
    Ok(s.to_owned())
}
fn uuid(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id)
        .ok()
        .is_none_or(|u| u.to_string() != id)
    {
        return Err(invalid());
    }
    Ok(())
}
fn read_permit(r: &Saved) -> ReadPermit {
    ReadPermit {
        source: r.view.source_session_id.clone(),
        admin: r.view.admin_session_id.clone(),
        captain: r.view.captain_session_id.clone(),
        case: r.view.case_id.clone(),
        crew: r.view.crew_id.clone(),
    }
}
fn bound(c: &Connection, r: &Saved) -> Result<execution::ExecutionWitness> {
    let w = execution::binding(c, &r.view.crew_id, &r.view.case_id)?.ok_or_else(invalid)?;
    if w.session_id() != r.view.admin_session_id || w.attempt_id() != r.attempt {
        return Err(invalid());
    }
    Ok(w)
}
fn captain_context(c: &Connection, crew: &str, cap: &str) -> Result<String> {
    let s = crate::sessions::get(c, cap)?;
    if s.role != Role::Captain
        || s.crew_id != crew
        || crate::crews::captain_of(c, crew)?.as_deref() != Some(cap)
        || wakes::automatic_work_paused(c, cap)?
    {
        return Err(invalid());
    }
    crate::crews::refuse_archived(c, crew)?;
    judgments::fingerprint(&(
        crew,
        cap,
        judgments::stop_epoch(c, cap)?,
        wakes::human_stop_floor(c, cap)?,
    ))
}
fn authority_current(c: &Connection, r: &Saved) -> Result<bool> {
    let w = bound(c, r)?;
    let a = execution::outcome_authority(c, &w)?;
    let original = admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: r.view.crew_id.clone(),
            id: r.view.case_id.clone(),
        },
    )?;
    if original.targets.len() != r.view.targets.len()
        || original.targets.iter().zip(&r.view.targets).any(|(a, b)| {
            a.id != b.id || a.crew_id != b.crew_id || a.kind != b.kind || a.scope != b.scope
        })
    {
        return Ok(false);
    }
    for t in r
        .view
        .targets
        .iter()
        .filter(|t| t.scope == AdminCaseScope::InScope)
    {
        match crate::work::get(c, &t.id) {
            Ok(w) if w.crew_id == r.view.crew_id => {}
            Ok(_) | Err(crate::CollabError::NotFound(_)) => return Ok(false),
            Err(e) => return Err(e),
        }
    }
    if r.view.phase == Phase::Accepted
        && crate::sessions::get(c, &r.view.captain_session_id)?
            .resume_failed_at
            .is_some()
    {
        return Ok(false);
    }
    Ok(a.current
        && a.actually_started
        && a.fingerprint == r.authority
        && captain_context(c, &r.view.crew_id, &r.view.captain_session_id)
            .ok()
            .as_deref()
            == Some(&r.captain_context))
}
fn pending_current(c: &Connection, r: &Saved, now: i64) -> Result<bool> {
    if !authority_current(c, r)? {
        return Ok(false);
    }
    let w = bound(c, r)?;
    Ok(execution::handoff_fingerprint(c, &w, now).ok().as_deref() == Some(&r.checkpoint))
}
fn notification(c: &Connection, r: &Saved) -> Result<Option<wakes::WakeRow>> {
    let row = wakes::by_ids(c, &[r.view.wake_id])?.into_iter().next();
    if let Some(w) = &row
        && (w.kind != wakes::WakeKind::System
            || w.priority != wakes::WakePriority::Auto
            || w.session != r.view.captain_session_id
            || w.source != format!("admin-case-handoff:{}", r.view.request_id)
            || w.origin.as_deref() != Some(&r.view.admin_session_id)
            || w.note.is_some())
    {
        return Err(invalid());
    }
    Ok(row)
}
fn project(c: &Connection, r: &Saved, now: i64) -> Result<AdminCaseHandoffView> {
    let mut v = r.view.clone();
    let valid = if v.phase == Phase::Accepted {
        authority_current(c, r)?
    } else {
        pending_current(c, r, now)?
    };
    if !valid {
        v.phase = Phase::Stale;
        v.reason = Reason::StateChanged;
        return Ok(v);
    }
    if v.phase == Phase::Accepted {
        return Ok(v);
    }
    match notification(c, r)? {
        Some(w)
            if w.state == wakes::WakeState::Delivered
                && w.attempts > 0
                && w.settled_at.is_some() =>
        {
            v.phase = Phase::Delivered;
            v.reason = Reason::NotificationDelivered;
            v.delivered_at = w.settled_at
        }
        Some(w)
            if matches!(
                w.state,
                wakes::WakeState::Pending | wakes::WakeState::Inflight
            ) => {}
        _ => {
            v.phase = Phase::Stale;
            v.reason = Reason::NotificationDropped
        }
    }
    v.revision = judgments::fingerprint(&(&r.view.revision, &v.phase, &v.delivered_at))?;
    Ok(v)
}
fn access(c: &Connection, by: &Sender, r: &Saved) -> Result<()> {
    let s = actor(c, by)?;
    if (s.id == r.view.admin_session_id && s.role == Role::Admin && s.crew_id == r.view.crew_id)
        || (s.id == r.view.captain_session_id && s.crew_id == r.view.crew_id)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}
pub fn get(c: &Connection, by: &Sender, id: &str, now: i64) -> Result<AdminCaseHandoffView> {
    uuid(id)?;
    let r = load(c, id)?.ok_or_else(invalid)?;
    access(c, by, &r)?;
    project(c, &r, now)
}
pub fn writes_relinquished(c: &Connection, case: &str) -> Result<bool> {
    let Some(id) = meta_get(c, &relinquished_key(case))? else {
        if let Some(active) = meta_get(c, &active_key(case))? {
            // load also detects an accepted receipt whose surrender marker was lost.
            let _ = load(c, &active)?.ok_or_else(invalid)?;
        }
        return Ok(false);
    };
    let r = load(c, &id)?.ok_or_else(invalid)?;
    if r.view.case_id != case || r.view.phase != Phase::Accepted {
        return Err(invalid());
    }
    Ok(true)
}
fn request_authority(
    c: &Connection,
    by: &Sender,
    id: &str,
    reason: &str,
    now: i64,
) -> Result<ReadPermit> {
    uuid(id)?;
    let reason = normalized(reason)?;
    let s = actor(c, by)?;
    if s.role != Role::Admin {
        return Err(invalid());
    }
    let w = execution::session_binding(c, &s.id)?.ok_or_else(invalid)?;
    if let Some(r) = load(c, id)? {
        if r.view.admin_session_id != s.id
            || r.attempt != w.attempt_id()
            || r.view.request_reason != reason
        {
            return Err(invalid());
        }
        return Ok(read_permit(&r));
    }
    if writes_relinquished(c, w.case_id())? {
        return Err(invalid());
    }
    execution::handoff_fingerprint(c, &w, now)?;
    if let Some(old) = meta_get(c, &active_key(w.case_id()))? {
        let r = load(c, &old)?.ok_or_else(invalid)?;
        if project(c, &r, now)?.phase != Phase::Stale {
            return Err(invalid());
        }
    }
    let cap = crate::crews::captain_of(c, w.crew_id())?.ok_or_else(invalid)?;
    verify_recipient(c, &cap)?;
    captain_context(c, w.crew_id(), &cap)?;
    Ok(ReadPermit {
        source: w.source_session_id().into(),
        admin: s.id,
        captain: cap,
        case: w.case_id().into(),
        crew: w.crew_id().into(),
    })
}
fn accept_authority(
    c: &Connection,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
    now: i64,
) -> Result<ReadPermit> {
    uuid(id)?;
    let note = normalized(note)?;
    let r = load(c, id)?.ok_or_else(invalid)?;
    let s = actor(c, by)?;
    if s.id != r.view.captain_session_id
        || s.resume_failed_at.is_some()
        || s.role != Role::Captain
        || s.crew_id != r.view.crew_id
        || !authority_current(c, &r)?
    {
        return Err(invalid());
    }
    if r.view.phase == Phase::Accepted {
        if r.accepted_revision.as_deref() != Some(revision)
            || r.view.acceptance_note.as_deref() != Some(&note)
        {
            return Err(invalid());
        }
        return Ok(read_permit(&r));
    }
    let v = project(c, &r, now)?;
    if v.phase != Phase::Delivered || v.revision != revision {
        return Err(invalid());
    }
    Ok(read_permit(&r))
}
pub fn validate_operation(
    c: &Connection,
    by: &Sender,
    op: &Operation,
    now: i64,
) -> Result<ReadPermit> {
    match op {
        Operation::Request { request_id, reason } => {
            request_authority(c, by, request_id, reason, now)
        }
        Operation::Accept {
            request_id,
            expected_revision,
            note,
        } => accept_authority(c, by, request_id, expected_revision, note, now),
        Operation::Get { request_id } => {
            let r = load(c, request_id)?.ok_or_else(invalid)?;
            access(c, by, &r)?;
            Ok(read_permit(&r))
        }
        Operation::List { after, limit } => {
            let s = list_actor(c, by)?;
            parse_cursor(c, &s.id, after.as_deref(), *limit)?;
            Ok(ReadPermit {
                source: String::new(),
                admin: String::new(),
                captain: s.id,
                case: String::new(),
                crew: s.crew_id,
            })
        }
    }
}
pub fn request(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    reason: &str,
) -> Result<AdminCaseHandoffView> {
    let p = request_authority(tx.conn(), by, id, reason, tx.now_ms())?;
    if let Some(r) = load(tx.conn(), id)? {
        return project(tx.conn(), &r, tx.now_ms());
    }
    let w = execution::session_binding(tx.conn(), &p.admin)?.ok_or_else(invalid)?;
    let snapshot = admin_cases::snapshot(tx.conn(), &p.crew, &p.case, tx.now_ms())?;
    let mut targets = snapshot.case.targets;
    for t in &mut targets {
        if t.scope == AdminCaseScope::InScope {
            t.revision = crate::work::get(tx.conn(), &t.id)?.revision;
        }
    }
    const COUNTER: &str = "admin.case.handoff.counter.v1";
    let sequence = match meta_get(tx.conn(), COUNTER)? {
        Some(v) => v
            .parse::<u64>()
            .map_err(|_| invalid())?
            .checked_add(1)
            .ok_or_else(invalid)?,
        None => {
            let exists: bool = tx.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key>=?1 AND key<?2)",
                params![PREFIX, format!("{PREFIX}\u{10ffff}")],
                |r| r.get(0),
            )?;
            if exists {
                return Err(invalid());
            }
            1
        }
    };
    let last: Option<String> = tx.conn().query_row(
        "SELECT MAX(key) FROM meta WHERE key>=?1 AND key<?2",
        params![GLOBAL, format!("{GLOBAL}\u{10ffff}")],
        |r| r.get(0),
    )?;
    if (sequence == 1 && last.is_some())
        || (sequence > 1
            && last.as_deref() != Some(format!("{GLOBAL}{:020}", sequence - 1).as_str()))
    {
        return Err(invalid());
    }
    let index = format!("{}{sequence:020}", index_prefix(&p.captain)?);
    if meta_get(tx.conn(), &index)?.is_some() {
        return Err(invalid());
    }
    let wake_id = match wakes::enqueue(
        tx,
        wakes::WakeRequest {
            session: p.captain.clone(),
            kind: wakes::WakeKind::System,
            source: format!("admin-case-handoff:{id}"),
            priority: wakes::WakePriority::Auto,
            not_before_ms: None,
            note: None,
            origin: Some(p.admin.clone()),
        },
    )? {
        wakes::Enqueued::New(id) => id,
        _ => return Err(invalid()),
    };
    let view = AdminCaseHandoffView {
        request_id: id.into(),
        case_id: p.case.clone(),
        crew_id: p.crew.clone(),
        revision: uuid::Uuid::new_v4().to_string(),
        phase: Phase::Waiting,
        reason: Reason::Requested,
        admin_session_id: p.admin,
        source_session_id: p.source,
        captain_session_id: p.captain,
        request_reason: normalized(reason)?,
        targets,
        wake_id,
        created_at: tx.now_ms(),
        delivered_at: None,
        accepted_at: None,
        acceptance_note: None,
    };
    let mut r = Saved {
        version: 1,
        sequence,
        attempt: w.attempt_id().into(),
        authority: execution::outcome_authority(tx.conn(), &w)?.fingerprint,
        checkpoint: execution::handoff_fingerprint(tx.conn(), &w, tx.now_ms())?,
        captain_context: captain_context(tx.conn(), &view.crew_id, &view.captain_session_id)?,
        view,
        accepted_revision: None,
        checksum: String::new(),
    };
    save(tx.conn(), &mut r)?;
    meta_set(tx.conn(), COUNTER, &sequence.to_string())?;
    meta_set(tx.conn(), &index, id)?;
    meta_set(tx.conn(), &format!("{GLOBAL}{sequence:020}"), id)?;
    meta_set(tx.conn(), &active_key(&p.case), id)?;
    save_recipient(tx.conn(), &r.view.captain_session_id)?;
    project(tx.conn(), &r, tx.now_ms())
}
pub fn accept(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
) -> Result<AdminCaseHandoffView> {
    accept_authority(tx.conn(), by, id, revision, note, tx.now_ms())?;
    let mut r = load(tx.conn(), id)?.ok_or_else(invalid)?;
    if r.view.phase == Phase::Accepted {
        return project(tx.conn(), &r, tx.now_ms());
    }
    r.view.delivered_at = project(tx.conn(), &r, tx.now_ms())?.delivered_at;
    r.view.phase = Phase::Accepted;
    r.view.reason = Reason::CaptainAccepted;
    r.view.accepted_at = Some(tx.now_ms());
    r.view.acceptance_note = Some(normalized(note)?);
    r.accepted_revision = Some(revision.into());
    r.view.revision = uuid::Uuid::new_v4().to_string();
    save(tx.conn(), &mut r)?;
    meta_set(tx.conn(), &relinquished_key(&r.view.case_id), id)?;
    Ok(r.view)
}
fn seq(c: &Connection) -> Result<u64> {
    Ok(crate::asks::load_org(c)?.state().last_seq)
}
pub fn begin_request(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    reason: &str,
) -> Result<RequestPermit> {
    Ok(RequestPermit {
        read: request_authority(tx.conn(), by, id, reason, tx.now_ms())?,
        operation: Operation::Request {
            request_id: id.into(),
            reason: reason.into(),
        },
        before_seq: seq(tx.conn())?,
    })
}
pub fn finish_request(tx: &mut Tx<'_>, p: RequestPermit) -> Result<()> {
    if seq(tx.conn())? != p.before_seq {
        return Err(invalid());
    }
    let Operation::Request { request_id, reason } = p.operation else {
        return Err(invalid());
    };
    let r = load(tx.conn(), &request_id)?.ok_or_else(invalid)?;
    if r.view.request_reason != normalized(&reason)?
        || r.view.admin_session_id != p.read.admin
        || r.view.captain_session_id != p.read.captain
        || r.view.case_id != p.read.case
        || notification(tx.conn(), &r)?.is_none()
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn begin_accept(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
) -> Result<AcceptPermit> {
    Ok(AcceptPermit {
        read: accept_authority(tx.conn(), by, id, revision, note, tx.now_ms())?,
        operation: Operation::Accept {
            request_id: id.into(),
            expected_revision: revision.into(),
            note: note.into(),
        },
        before_seq: seq(tx.conn())?,
    })
}
pub fn finish_accept(tx: &mut Tx<'_>, p: AcceptPermit) -> Result<()> {
    if seq(tx.conn())? != p.before_seq {
        return Err(invalid());
    }
    let Operation::Accept {
        request_id,
        expected_revision,
        note,
    } = p.operation
    else {
        return Err(invalid());
    };
    let r = load(tx.conn(), &request_id)?.ok_or_else(invalid)?;
    if r.view.phase != Phase::Accepted
        || r.accepted_revision.as_deref() != Some(&expected_revision)
        || r.view.acceptance_note.as_deref() != Some(&normalized(&note)?)
        || r.view.captain_session_id != p.read.captain
        || r.view.case_id != p.read.case
        || !writes_relinquished(tx.conn(), &p.read.case)?
    {
        return Err(invalid());
    }
    Ok(())
}
fn list_actor(c: &Connection, by: &Sender) -> Result<Session> {
    let s = actor(c, by)?;
    verify_recipient(c, &s.id)?;
    if s.role != Role::Captain {
        let prefix = index_prefix(&s.id)?;
        let present: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key>=?1 AND key<?2)",
            params![prefix, format!("{prefix}\u{10ffff}")],
            |r| r.get(0),
        )?;
        if !present {
            return Err(invalid());
        }
    }
    Ok(s)
}

fn parse_cursor(c: &Connection, caller: &str, after: Option<&str>, limit: u32) -> Result<u64> {
    if limit == 0 || limit > 100 {
        return Err(invalid());
    }
    let Some(after) = after else { return Ok(0) };
    let (witness, sequence) = after.rsplit_once(':').ok_or_else(invalid)?;
    if witness != judgments::fingerprint(&(caller, crewd_store::judgment_state::epoch(c)?))? {
        return Err(invalid());
    }
    let n = sequence.parse::<u64>().map_err(|_| invalid())?;
    if format!("{n:020}") != sequence {
        return Err(invalid());
    }
    Ok(n)
}
pub fn list(
    c: &Connection,
    by: &Sender,
    after: Option<String>,
    limit: u32,
    now: i64,
) -> Result<AdminCaseHandoffList> {
    let s = list_actor(c, by)?;
    let n = parse_cursor(c, &s.id, after.as_deref(), limit)?;
    let prefix = index_prefix(&s.id)?;
    let mut stmt =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let rows = stmt
        .query_map(
            params![
                format!("{prefix}{n:020}"),
                format!("{prefix}\u{10ffff}"),
                i64::from(limit) + 1
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    let mut last = 0;
    for (k, id) in rows.into_iter().take(limit as usize) {
        let r = load(c, &id)?.ok_or_else(invalid)?;
        if r.view.captain_session_id != s.id || k != format!("{prefix}{:020}", r.sequence) {
            return Err(invalid());
        }
        access(c, by, &r)?;
        last = r.sequence;
        items.push(project(c, &r, now)?);
    }
    let next_after = if more {
        Some(format!(
            "{}:{last:020}",
            judgments::fingerprint(&(&s.id, crewd_store::judgment_state::epoch(c)?))?
        ))
    } else {
        None
    };
    Ok(AdminCaseHandoffList { items, next_after })
}
pub fn status_for_case(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<AdminCaseHandoffView>> {
    let Some(request) = meta_get(c, &active_key(id))? else {
        return Ok(None);
    };
    let r = load(c, &request)?.ok_or_else(invalid)?;
    if r.view.crew_id != crew || r.view.case_id != id {
        return Err(invalid());
    }
    Ok(Some(project(c, &r, now)?))
}
pub(crate) fn current_accepted(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<AdminCaseHandoffView>> {
    Ok(status_for_case(c, crew, id, now)?.filter(|v| v.phase == Phase::Accepted))
}
/// Immutable original accepted responsibility, even when its authority is now stale.
pub(crate) fn custody_origin(
    c: &Connection,
    crew: &str,
    case: &str,
) -> Result<Option<AdminCaseHandoffView>> {
    let Some(id) = meta_get(c, &relinquished_key(case))? else {
        return Ok(None);
    };
    let r = load(c, &id)?.ok_or_else(invalid)?;
    if r.view.phase != Phase::Accepted || r.view.crew_id != crew || r.view.case_id != case {
        return Err(invalid());
    }
    Ok(Some(r.view))
}
pub(crate) fn has_unresolved_custody(c: &Connection, session: &str, now: i64) -> Result<bool> {
    if crate::admin_case_custody::has_unresolved_custody(c, session, now)? {
        return Ok(true);
    }
    verify_recipient(c, session)?;
    // Only this recipient's indexed history; stream 64 rows at a time. A stale
    // accepted responsibility remains an obligation until verified resolution.
    let prefix = index_prefix(session)?;
    let upper = format!("{prefix}\u{10ffff}");
    let mut after = prefix.clone();
    loop {
        let mut stmt =
            c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT 64")?;
        let rows = stmt
            .query_map(params![after, upper], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok(false);
        }
        for (key, id) in rows {
            let r = load(c, &id)?.ok_or_else(invalid)?;
            if r.view.captain_session_id != session || key != format!("{prefix}{:020}", r.sequence)
            {
                return Err(invalid());
            }
            if r.view.phase == Phase::Accepted
                && !crate::admin_case_custody::original_obligation_released(
                    c,
                    &r.view.crew_id,
                    &r.view.case_id,
                )?
                && crate::admin_case_outcome::status_at(c, &r.view.crew_id, &r.view.case_id, now)?
                    .is_none_or(|o| o.phase != AdminCaseOutcomePhase::Resolved)
            {
                return Ok(true);
            }
            after = key;
        }
    }
}

pub(crate) fn render(
    c: &Connection,
    row: &wakes::WakeRow,
    now: i64,
) -> Result<crate::work::FollowupNote> {
    let Some(id) = row.source.strip_prefix("admin-case-handoff:") else {
        return Ok(crate::work::FollowupNote::Ordinary);
    };
    let Some(r) = load(c, id)? else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    if notification(c, &r)?.as_ref() != Some(row)
        || !pending_current(c, &r, now)?
        || r.view.phase != Phase::Waiting
    {
        return Ok(crate::work::FollowupNote::Stale);
    }
    let targets = r
        .view
        .targets
        .iter()
        .filter(|t| t.scope == AdminCaseScope::InScope)
        .map(|t| t.id.as_str())
        .take(50)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(crate::work::FollowupNote::Current(format!(
        "行政事项请求你明确承接。request_id={}；原范围工作：{}（正文最多50项，完整范围请用 admin_case_handoff_get）。原因：{}。通知送达不代表承接或工作完成；查看后使用 admin_case_handoff_accept 和当前 revision 明确承接。",
        id, targets, r.view.request_reason
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_case_execution::tests::{close, running_case};
    use crate::testkit::{NOW, ok, store};
    fn setup() -> (
        crewd_store::Store,
        AdminCaseView,
        execution::Registration,
        AdminCaseHandoffView,
    ) {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        let id = uuid::Uuid::new_v4().to_string();
        let v = ok(&mut s, |tx| {
            request(
                tx,
                &Sender::Session(reg.session.id.clone()),
                &id,
                "need captain coordination",
            )
        });
        (s, case, reg, v)
    }
    fn delivered(s: &mut crewd_store::Store, v: &AdminCaseHandoffView) -> AdminCaseHandoffView {
        ok(s, |tx| {
            wakes::claim(tx, &[v.wake_id])?;
            wakes::ack_delivered(
                tx,
                &v.captain_session_id,
                &[v.wake_id],
                wakes::Riders::Release,
                &[],
                &[],
            )
        });
        get(
            s.conn(),
            &Sender::Session(v.captain_session_id.clone()),
            &v.request_id,
            NOW,
        )
        .unwrap()
    }
    #[test]
    fn request_uuid_body_and_transaction_rollback() {
        let (mut s, _, reg, v) = setup();
        let by = Sender::Session(reg.session.id);
        assert_eq!(
            ok(&mut s, |tx| request(
                tx,
                &by,
                &v.request_id,
                " need captain coordination "
            ))
            .wake_id,
            v.wake_id
        );
        assert!(
            crate::run(&mut s, NOW, |tx| request(
                tx,
                &by,
                &v.request_id,
                "different"
            ))
            .is_err()
        );
        let (_, _, _, _) = (&s, &by, &v, &reg.witness);
        let mut other = store();
        let (_, r) = running_case(&mut other);
        let id = uuid::Uuid::new_v4().to_string();
        assert!(
            crate::run::<()>(&mut other, NOW, |tx| {
                request(tx, &Sender::Session(r.session.id), &id, "rollback")?;
                Err(invalid())
            })
            .is_err()
        );
        assert!(meta_get(other.conn(), &key(&id)).unwrap().is_none());
        let count: i64 = other
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM wakes WHERE source=?1",
                [format!("admin-case-handoff:{id}")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    #[test]
    fn delivered_is_not_accepted_and_accepted_cuts_write_permanently() {
        let (mut s, case, reg, v) = setup();
        let cap = Sender::Session(v.captain_session_id.clone());
        assert!(
            crate::run(&mut s, NOW, |tx| accept(
                tx,
                &cap,
                &v.request_id,
                &v.revision,
                "take custody"
            ))
            .is_err()
        );
        let v = delivered(&mut s, &v);
        assert_eq!(v.phase, Phase::Delivered);
        assert!(!writes_relinquished(s.conn(), &case.id).unwrap());
        let a = ok(&mut s, |tx| {
            accept(tx, &cap, &v.request_id, &v.revision, "take custody")
        });
        assert_eq!(a.phase, Phase::Accepted);
        assert!(has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        assert!(
            crate::run(&mut s, NOW, |tx| execution::validate_effect(
                tx,
                &reg.witness,
                execution::ActionScope::WorkClose {
                    id: case.targets[0].id.clone(),
                    kind: WorkClosureKind::Done
                }
            ))
            .is_err()
        );
        ok(&mut s, |tx| {
            wakes::human_stop(tx, "w-a1")?;
            Ok(())
        });
        assert_eq!(
            get(s.conn(), &cap, &v.request_id, NOW).unwrap().phase,
            Phase::Stale
        );
        assert!(has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        assert!(writes_relinquished(s.conn(), &case.id).unwrap());
    }
    #[test]
    fn captain_progress_does_not_erase_accepted_custody() {
        let (mut s, case, _, v) = setup();
        let v = delivered(&mut s, &v);
        let cap = Sender::Session("cap-a".into());
        ok(&mut s, |tx| {
            accept(tx, &cap, &v.request_id, &v.revision, "take custody")
        });
        ok(&mut s, |tx| close(tx, "cap-a", &case.targets[0].id));
        assert_eq!(
            get(s.conn(), &cap, &v.request_id, NOW).unwrap().phase,
            Phase::Accepted
        );
        assert!(
            current_accepted(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn checksum_tamper_is_rejected_and_raw_preserved() {
        let (s, _, _, v) = setup();
        let raw = meta_get(s.conn(), &key(&v.request_id)).unwrap().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["view"]["phase"] = "accepted".into();
        let bad = value.to_string();
        meta_set(s.conn(), &key(&v.request_id), &bad).unwrap();
        assert!(
            get(
                s.conn(),
                &Sender::Session("cap-a".into()),
                &v.request_id,
                NOW
            )
            .is_err()
        );
        assert_eq!(meta_get(s.conn(), &key(&v.request_id)).unwrap(), Some(bad));
    }
    #[test]
    fn former_captain_can_find_custody_but_cannot_accept() {
        let (mut s, _, _, v) = setup();
        let v = delivered(&mut s, &v);
        s.conn()
            .execute("UPDATE sessions SET role='worker' WHERE id='cap-a'", [])
            .unwrap();
        let by = Sender::Session("cap-a".into());
        assert_eq!(
            get(s.conn(), &by, &v.request_id, NOW).unwrap().phase,
            Phase::Stale
        );
        assert_eq!(list(s.conn(), &by, None, 50, NOW).unwrap().items.len(), 1);
        assert!(
            crate::run(&mut s, NOW, |tx| accept(
                tx,
                &by,
                &v.request_id,
                &v.revision,
                "take custody"
            ))
            .is_err()
        );
        assert!(list(s.conn(), &Sender::Session("w-a2".into()), None, 50, NOW).is_err());
    }
    #[test]
    fn missing_counter_cannot_reuse_identity() {
        let (mut s, _, reg, v) = setup();
        ok(&mut s, |tx| wakes::mark_dropped(tx, v.wake_id, "synthetic"));
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key='admin.case.handoff.counter.v1'",
                [],
            )
            .unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| request(
                tx,
                &Sender::Session(reg.session.id),
                &uuid::Uuid::new_v4().to_string(),
                "new"
            ))
            .is_err()
        );
    }
    #[test]
    fn pages_are_monotonic_and_epoch_bound() {
        let (mut s, _, reg, first) = setup();
        let by = Sender::Session(reg.session.id.clone());
        ok(&mut s, |tx| {
            wakes::mark_dropped(tx, first.wake_id, "superseded synthetic notice")
        });
        let second = ok(&mut s, |tx| {
            request(tx, &by, &uuid::Uuid::new_v4().to_string(), "second")
        });
        let cap = Sender::Session("cap-a".into());
        let page = list(s.conn(), &cap, None, 1, NOW).unwrap();
        assert_eq!(page.items[0].request_id, first.request_id);
        let cursor = page.next_after.unwrap();
        assert_eq!(
            list(s.conn(), &cap, Some(cursor.clone()), 1, NOW)
                .unwrap()
                .items[0]
                .request_id,
            second.request_id
        );
        assert!(
            list(
                s.conn(),
                &Sender::Session("cap-b".into()),
                Some(cursor.clone()),
                1,
                NOW
            )
            .is_err()
        );
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(list(s.conn(), &cap, Some(cursor), 1, NOW).is_err());
    }
    #[test]
    fn lost_surrender_marker_does_not_restore_admin_authority() {
        let (mut s, case, _, v) = setup();
        let v = delivered(&mut s, &v);
        ok(&mut s, |tx| {
            accept(
                tx,
                &Sender::Session("cap-a".into()),
                &v.request_id,
                &v.revision,
                "take custody",
            )
        });
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [relinquished_key(&case.id)],
            )
            .unwrap();
        assert!(writes_relinquished(s.conn(), &case.id).is_err());
    }
    #[test]
    fn verified_external_resolution_discharges_captain_custody() {
        let (mut s, case, _, v) = setup();
        let v = delivered(&mut s, &v);
        ok(&mut s, |tx| {
            accept(
                tx,
                &Sender::Session("cap-a".into()),
                &v.request_id,
                &v.revision,
                "take custody",
            )
        });
        for target in case.targets {
            ok(&mut s, |tx| close(tx, "cap-a", &target.id));
        }
        assert!(has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        let outcome = ok(&mut s, |tx| {
            crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
        });
        assert_eq!(outcome.phase, AdminCaseOutcomePhase::Resolved);
        assert!(outcome.resolved_by_admin.is_empty());
        assert!(!has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        assert!(writes_relinquished(s.conn(), &case.id).unwrap());
    }
    #[test]
    fn missing_recipient_index_is_not_no_custody() {
        let (mut s, _, _, v) = setup();
        let v = delivered(&mut s, &v);
        ok(&mut s, |tx| {
            accept(
                tx,
                &Sender::Session("cap-a".into()),
                &v.request_id,
                &v.revision,
                "take custody",
            )
        });
        let prefix = index_prefix("cap-a").unwrap();
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key>=?1 AND key<?2",
                params![prefix, format!("{prefix}\u{10ffff}")],
            )
            .unwrap();
        assert!(
            has_unresolved_custody(s.conn(), "cap-a", NOW).is_err(),
            "lost index must not make accepted custody disappear"
        );
        assert!(list(s.conn(), &Sender::Session("cap-a".into()), None, 50, NOW).is_err());
    }
    #[test]
    fn recipient_witness_and_marker_corruption_fail_closed_without_repair() {
        let (s, _, _, _) = setup();
        let (key, marker) = recipient_keys("cap-a").unwrap();
        let raw = meta_get(s.conn(), &key).unwrap().unwrap();
        for removed in [&key, &marker] {
            let previous = meta_get(s.conn(), removed).unwrap().unwrap();
            s.conn()
                .execute("DELETE FROM meta WHERE key=?1", [removed])
                .unwrap();
            assert!(has_unresolved_custody(s.conn(), "cap-a", NOW).is_err());
            assert!(list(s.conn(), &Sender::Session("cap-a".into()), None, 50, NOW).is_err());
            assert!(meta_get(s.conn(), removed).unwrap().is_none());
            meta_set(s.conn(), removed, &previous).unwrap();
        }
        let mut bad: serde_json::Value = serde_json::from_str(&raw).unwrap();
        bad["count"] = 0.into();
        let bad = bad.to_string();
        meta_set(s.conn(), &key, &bad).unwrap();
        assert!(has_unresolved_custody(s.conn(), "cap-a", NOW).is_err());
        assert_eq!(meta_get(s.conn(), &key).unwrap(), Some(bad));
    }
}
