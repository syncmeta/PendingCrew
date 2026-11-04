//! Explicit, durable succession of administrative custody; never execution authority.
use crate::{Result, Sender, Tx, admin_case_handoff as handoff, judgments, wakes};
use AdminCaseCustodyPhase as Phase;
use AdminCaseCustodyReason as Reason;
use AdminCaseCustodyTransferPhase as TransferPhase;
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
const PREFIX: &str = "admin.case.custody.transfer.v1:";
const COUNTER: &str = "admin.case.custody.counter.v1";
const GLOBAL: &str = "admin.case.custody.sequence.v1:";
fn invalid() -> crate::CollabError {
    crate::resource_acl::invalid("admin_case_custody")
}
fn key(id: &str) -> String {
    format!("{PREFIX}{id}")
}
fn state_key(id: &str) -> String {
    format!("admin.case.custody.state.v1:{id}")
}
fn marker_key(id: &str) -> String {
    format!("admin.case.custody.present.v1:{id}")
}
fn text(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > 2000 {
        return Err(invalid());
    }
    Ok(s.into())
}
fn uuid(s: &str) -> Result<()> {
    if uuid::Uuid::parse_str(s)
        .ok()
        .is_none_or(|u| u.to_string() != s)
    {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    version: u32,
    sequence: u64,
    authority: String,
    proposal_revision: String,
    accepted_revision: Option<String>,
    view: AdminCaseCustodyTransfer,
    checksum: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Discharge {
    receipt: String,
    authority: String,
    facts: String,
    view: AdminCaseCustodyDischarge,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    case: String,
    crew: String,
    origin: String,
    original_digest: String,
    first: Option<String>,
    current: Option<String>,
    pending: Option<String>,
    accepted_count: u64,
    revision: String,
    discharge: Option<Discharge>,
    checksum: String,
}
fn transfer_digest(t: &Transfer) -> Result<String> {
    judgments::fingerprint(&(
        t.version,
        t.sequence,
        &t.authority,
        &t.proposal_revision,
        &t.accepted_revision,
        &t.view,
    ))
}
fn state_digest(s: &State) -> Result<String> {
    judgments::fingerprint(&(
        s.version,
        &s.case,
        &s.crew,
        &s.origin,
        &s.original_digest,
        &s.first,
        &s.current,
        &s.pending,
        s.accepted_count,
        &s.revision,
        &s.discharge,
    ))
}
fn save_transfer(c: &Connection, t: &mut Transfer) -> Result<()> {
    t.checksum = transfer_digest(t)?;
    meta_set(
        c,
        &key(&t.view.request_id),
        &serde_json::to_string(t).map_err(StoreError::from)?,
    )?;
    Ok(())
}
fn save_state(c: &Connection, s: &mut State) -> Result<()> {
    s.checksum = state_digest(s)?;
    meta_set(
        c,
        &state_key(&s.case),
        &serde_json::to_string(s).map_err(StoreError::from)?,
    )?;
    meta_set(c, &marker_key(&s.case), "1")?;
    Ok(())
}
fn recipient_prefix(id: &str) -> Result<String> {
    Ok(format!(
        "admin.case.custody.recipient.v1:{}:",
        judgments::fingerprint(&id)?
    ))
}
fn recipient_witness_key(id: &str) -> Result<String> {
    Ok(format!(
        "admin.case.custody.recipientWitness.v1:{}",
        judgments::fingerprint(&id)?
    ))
}
fn recipient_marker_key(id: &str) -> Result<String> {
    Ok(format!(
        "admin.case.custody.recipientPresent.v1:{}",
        judgments::fingerprint(&id)?
    ))
}
fn recipient_digest(c: &Connection, id: &str) -> Result<(u64, String)> {
    let prefix = recipient_prefix(id)?;
    let mut q = c.prepare("SELECT key,value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")?;
    let mut rows = q.query(params![prefix, format!("{prefix}\u{10ffff}")])?;
    let mut count = 0u64;
    let mut hash = judgments::fingerprint(&("custody-index", id))?;
    while let Some(r) = rows.next()? {
        let k: String = r.get(0)?;
        let v: String = r.get(1)?;
        count = count.checked_add(1).ok_or_else(invalid)?;
        hash = judgments::fingerprint(&(&hash, k, v))?;
    }
    Ok((count, hash))
}
fn verify_recipient(c: &Connection, id: &str) -> Result<()> {
    let actual = recipient_digest(c, id)?;
    let raw = meta_get(c, &recipient_witness_key(id)?)?;
    let marker = meta_get(c, &recipient_marker_key(id)?)?;
    match (raw, marker) {
        (None, None) if actual.0 == 0 => Ok(()),
        (Some(raw), Some(marker)) if marker == "1" => {
            let saved: (u64, String) = serde_json::from_str(&raw).map_err(StoreError::from)?;
            if actual != saved || actual.0 == 0 {
                return Err(invalid());
            }
            Ok(())
        }
        _ => Err(invalid()),
    }
}
fn save_recipient(c: &Connection, id: &str) -> Result<()> {
    meta_set(
        c,
        &recipient_witness_key(id)?,
        &serde_json::to_string(&recipient_digest(c, id)?).map_err(StoreError::from)?,
    )?;
    meta_set(c, &recipient_marker_key(id)?, "1")?;
    Ok(())
}
fn load_transfer(c: &Connection, id: &str) -> Result<Option<Transfer>> {
    let Some(raw) = meta_get(c, &key(id))? else {
        return Ok(None);
    };
    let t: Transfer = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if t.version != 1
        || t.sequence == 0
        || t.view.request_id != id
        || transfer_digest(&t)? != t.checksum
        || !matches!(
            t.view.phase,
            TransferPhase::Waiting | TransferPhase::Accepted
        )
        || (t.view.phase == TransferPhase::Accepted
            && (t.accepted_revision.is_none()
                || t.view.delivered_at.is_none()
                || t.view.accepted_at.is_none()
                || t.view.acceptance_note.is_none()))
        || meta_get(c, &format!("{GLOBAL}{:020}", t.sequence))?.as_deref() != Some(id)
    {
        return Err(invalid());
    }
    for recipient in [&t.view.from_session_id, &t.view.to_session_id] {
        verify_recipient(c, recipient)?;
        if meta_get(
            c,
            &format!("{}{:020}", recipient_prefix(recipient)?, t.sequence),
        )?
        .as_deref()
            != Some(id)
        {
            return Err(invalid());
        }
    }
    Ok(Some(t))
}
fn origin(c: &Connection, crew: &str, case: &str) -> Result<AdminCaseHandoffView> {
    handoff::custody_origin(c, crew, case)?.ok_or_else(invalid)
}
fn load_state(c: &Connection, o: &AdminCaseHandoffView) -> Result<Option<State>> {
    let raw = meta_get(c, &state_key(&o.case_id))?;
    let marker = meta_get(c, &marker_key(&o.case_id))?;
    let (Some(raw), Some(marker)) = (raw.clone(), marker.clone()) else {
        if raw.is_some() || marker.is_some() {
            return Err(invalid());
        }
        return Ok(None);
    };
    let s: State = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if marker != "1"
        || s.version != 1
        || s.case != o.case_id
        || s.crew != o.crew_id
        || s.origin != o.request_id
        || s.original_digest != judgments::fingerprint(o)?
        || s.checksum != state_digest(&s)?
    {
        return Err(invalid());
    }
    let mut next = s.first.clone();
    let mut prior = o.request_id.clone();
    let mut holder = o.captain_session_id.clone();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = next {
        if !seen.insert(id.clone()) || seen.len() as u64 > s.accepted_count {
            return Err(invalid());
        }
        let t = load_transfer(c, &id)?.ok_or_else(invalid)?;
        if t.view.case_id != s.case
            || t.view.crew_id != s.crew
            || t.view.phase != TransferPhase::Accepted
            || t.view.predecessor_receipt_id != prior
            || t.view.from_session_id != holder
        {
            return Err(invalid());
        }
        prior = id;
        holder = t.view.to_session_id;
        next = t.view.superseded_by;
    }
    if seen.len() as u64 != s.accepted_count
        || (s.accepted_count == 0 && s.current.is_some())
        || (s.accepted_count > 0 && s.current.as_deref() != Some(&prior))
    {
        return Err(invalid());
    }
    if let Some(id) = &s.pending {
        let t = load_transfer(c, id)?.ok_or_else(invalid)?;
        if t.view.phase != TransferPhase::Waiting
            || t.view.predecessor_receipt_id != prior
            || t.view.from_session_id != holder
            || t.view.case_id != s.case
            || t.view.crew_id != s.crew
            || t.view.superseded_by.is_some()
        {
            return Err(invalid());
        }
    }
    Ok(Some(s))
}
fn current_holder(
    c: &Connection,
    o: &AdminCaseHandoffView,
    s: Option<&State>,
) -> Result<(String, String)> {
    if let Some(id) = s.and_then(|s| s.current.as_ref()) {
        let t = load_transfer(c, id)?.ok_or_else(invalid)?;
        Ok((id.clone(), t.view.to_session_id))
    } else {
        Ok((o.request_id.clone(), o.captain_session_id.clone()))
    }
}
fn environment(c: &Connection, crew: &str, cap: &str) -> Result<String> {
    crate::crews::refuse_archived(c, crew)?;
    let s = crate::sessions::get(c, cap)?;
    if s.crew_id != crew
        || s.role != Role::Captain
        || crate::crews::captain_of(c, crew)?.as_deref() != Some(cap)
        || s.resume_failed_at.is_some()
        || wakes::automatic_work_paused(c, cap)?
    {
        return Err(invalid());
    }
    judgments::fingerprint(&(
        crewd_store::judgment_state::epoch(c)?,
        judgments::configuration(c)?,
        crew,
        cap,
        judgments::stop_epoch(c, cap)?,
        wakes::human_stop_floor(c, cap)?,
    ))
}
fn target_scope(c: &Connection, o: &AdminCaseHandoffView) -> Result<Vec<AdminCaseTarget>> {
    let source = crate::admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: o.crew_id.clone(),
            id: o.case_id.clone(),
        },
    )?;
    if source.targets.len() != o.targets.len()
        || source.targets.iter().zip(&o.targets).any(|(a, b)| {
            a.id != b.id || a.crew_id != b.crew_id || a.kind != b.kind || a.scope != b.scope
        })
    {
        return Err(invalid());
    }
    let mut targets = o.targets.clone();
    for t in &mut targets {
        if t.scope == AdminCaseScope::InScope {
            let p = crate::work::get(c, &t.id)?;
            if p.crew_id != o.crew_id {
                return Err(invalid());
            }
            t.revision = p.revision;
        }
    }
    Ok(targets)
}
fn transfer_current(c: &Connection, t: &Transfer, o: &AdminCaseHandoffView) -> Result<bool> {
    if environment(c, &t.view.crew_id, &t.view.to_session_id)
        .ok()
        .as_deref()
        != Some(&t.authority)
    {
        return Ok(false);
    }
    let Ok(targets) = target_scope(c, o) else {
        return Ok(false);
    };
    Ok(if t.view.phase == TransferPhase::Accepted {
        true
    } else {
        targets == t.view.targets
    })
}
fn notification(c: &Connection, t: &Transfer) -> Result<Option<wakes::WakeRow>> {
    let row = wakes::by_ids(c, &[t.view.wake_id])?.into_iter().next();
    if let Some(w) = &row
        && (w.kind != wakes::WakeKind::System
            || w.session != t.view.to_session_id
            || w.source != format!("admin-case-custody:{}", t.view.request_id)
            || w.priority != wakes::WakePriority::Auto
            || w.origin.is_some()
            || w.note.is_some())
    {
        return Err(invalid());
    }
    Ok(row)
}
fn project_transfer(
    c: &Connection,
    t: &Transfer,
    o: &AdminCaseHandoffView,
) -> Result<AdminCaseCustodyTransfer> {
    let mut v = t.view.clone();
    if t.view.phase == TransferPhase::Waiting && t.view.superseded_by.is_some() {
        v.phase = TransferPhase::Stale;
        v.reason = Reason::Superseded;
    } else if !transfer_current(c, t, o)? {
        v.phase = TransferPhase::Stale;
        v.reason = Reason::StateChanged;
    } else if t.view.phase == TransferPhase::Waiting {
        match notification(c, t)? {
            Some(w)
                if w.state == wakes::WakeState::Delivered
                    && w.attempts > 0
                    && w.settled_at.is_some() =>
            {
                v.phase = TransferPhase::Delivered;
                v.reason = Reason::NotificationDelivered;
                v.delivered_at = w.settled_at;
            }
            Some(w)
                if matches!(
                    w.state,
                    wakes::WakeState::Pending | wakes::WakeState::Inflight
                ) => {}
            _ => {
                v.phase = TransferPhase::Stale;
                v.reason = Reason::NotificationDropped;
            }
        }
    }
    v.revision = judgments::fingerprint(&(
        &t.view.revision,
        &v.phase,
        &v.delivered_at,
        &v.superseded_by,
    ))?;
    Ok(v)
}
fn access(c: &Connection, by: &Sender, o: &AdminCaseHandoffView, s: Option<&State>) -> Result<()> {
    if matches!(by, Sender::Human) {
        return Ok(());
    }
    let Sender::Session(id) = by else {
        return Err(invalid());
    };
    let actor = crate::sessions::get(c, id)?;
    if actor.crew_id != o.crew_id {
        return Err(invalid());
    }
    let (_, holder) = current_holder(c, o, s)?;
    if id == &holder
        || id == &o.captain_session_id
        || id == &o.admin_session_id
        || (actor.role == Role::Captain
            && crate::crews::captain_of(c, &o.crew_id)?.as_ref() == Some(id))
    {
        return Ok(());
    }
    verify_recipient(c, id)?;
    let prefix = recipient_prefix(id)?;
    let mut q = c.prepare("SELECT value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")?;
    let mut rows = q.query(params![prefix, format!("{prefix}\u{10ffff}")])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let t = load_transfer(c, &id)?.ok_or_else(invalid)?;
        if t.view.case_id == o.case_id
            && (t.view.from_session_id == actor.id || t.view.to_session_id == actor.id)
        {
            return Ok(());
        }
    }
    Err(invalid())
}
fn business_facts(
    c: &Connection,
    crew: &str,
    case: &str,
    now: i64,
) -> Result<(AdminCaseOutcomeView, String, bool)> {
    let facts = crate::admin_case_outcome::custody_facts(c, crew, case, now)?;
    let fingerprint = judgments::fingerprint(&(&facts.targets, &facts.out_of_scope))?;
    let complete = !facts.targets.is_empty()
        && facts.out_of_scope.is_empty()
        && facts
            .targets
            .iter()
            .all(|t| t.state == AdminCaseTargetOutcomeState::Resolved);
    Ok((facts, fingerprint, complete))
}
fn discharge_valid(c: &Connection, s: &State, o: &AdminCaseHandoffView, now: i64) -> Result<bool> {
    let Some(d) = &s.discharge else {
        return Ok(false);
    };
    if s.current.as_deref() != Some(&d.receipt) {
        return Err(invalid());
    }
    let t = load_transfer(c, &d.receipt)?.ok_or_else(invalid)?;
    if !transfer_current(c, &t, o)? || t.authority != d.authority {
        return Ok(false);
    }
    let (facts, fingerprint, complete) = business_facts(c, &s.crew, &s.case, now)?;
    Ok(complete
        && fingerprint == d.facts
        && facts.targets == d.view.targets
        && d.view.revision == judgments::fingerprint(&(&d.receipt, &d.authority, &d.facts))?)
}
fn project(
    c: &Connection,
    o: &AdminCaseHandoffView,
    s: Option<&State>,
    now: i64,
) -> Result<AdminCaseCustodyView> {
    let (receipt, holder) = current_holder(c, o, s)?;
    let current = s
        .and_then(|s| s.current.as_ref())
        .map(|id| load_transfer(c, id)?.ok_or_else(invalid))
        .transpose()?;
    let pending = s
        .and_then(|s| s.pending.as_ref())
        .map(|id| load_transfer(c, id)?.ok_or_else(invalid))
        .transpose()?;
    let current_record = current
        .as_ref()
        .map(|t| project_transfer(c, t, o))
        .transpose()?;
    let targets = target_scope(c, o).ok();
    let valid = if let Some(t) = &current {
        transfer_current(c, t, o)?
    } else {
        handoff::status_for_case(c, &o.crew_id, &o.case_id, now)?
            .is_some_and(|v| v.phase == AdminCaseHandoffPhase::Accepted)
    };
    let discharge = if let Some(s) = s {
        if discharge_valid(c, s, o, now)? {
            s.discharge.as_ref().map(|d| d.view.clone())
        } else {
            None
        }
    } else {
        None
    };
    let phase = if discharge.is_some() {
        Phase::Discharged
    } else if valid {
        Phase::Active
    } else {
        Phase::Stale
    };
    let facts = crate::admin_case_outcome::custody_facts(c, &o.crew_id, &o.case_id, now)?;
    let reason = if discharge.is_some() {
        Reason::Verified
    } else if targets.is_none() {
        Reason::TargetMissing
    } else if !valid {
        Reason::StateChanged
    } else if !facts.out_of_scope.is_empty() {
        Reason::OutOfScope
    } else if !facts.missing.is_empty() {
        Reason::TargetMissing
    } else if !facts.blocked.is_empty() {
        Reason::EvidenceIncomplete
    } else if !facts.pending.is_empty() {
        Reason::TargetsUnresolved
    } else if current.is_some() {
        Reason::CaptainAccepted
    } else {
        Reason::OriginalCustody
    };
    let pending = pending
        .as_ref()
        .map(|t| project_transfer(c, t, o))
        .transpose()?;
    let cap = crate::crews::captain_of(c, &o.crew_id)?;
    let capenv = cap.as_deref().map(|id| environment(c, &o.crew_id, id).ok());
    let revision = judgments::fingerprint(&(
        s.map(|s| &s.checksum),
        &o.request_id,
        &receipt,
        &holder,
        &phase,
        &targets,
        &pending,
        cap,
        capenv,
        crewd_store::judgment_state::epoch(c)?,
        judgments::configuration(c)?,
    ))?;
    Ok(AdminCaseCustodyView {
        case_id: o.case_id.clone(),
        crew_id: o.crew_id.clone(),
        revision,
        original_request_id: o.request_id.clone(),
        current_custodian_session_id: holder,
        current_receipt_id: receipt,
        phase,
        reason,
        targets: targets.unwrap_or_else(|| o.targets.clone()),
        target_results: facts.targets,
        out_of_scope: facts.out_of_scope,
        pending,
        current_record,
        discharge,
    })
}
fn next_sequence(c: &Connection) -> Result<u64> {
    let last: Option<String> = c.query_row(
        "SELECT MAX(key) FROM meta WHERE key>=?1 AND key<?2",
        params![GLOBAL, format!("{GLOBAL}\u{10ffff}")],
        |r| r.get(0),
    )?;
    let previous = match meta_get(c, COUNTER)? {
        Some(v) => v.parse::<u64>().map_err(|_| invalid())?,
        None => {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key>=?1 AND key<?2)",
                params![PREFIX, format!("{PREFIX}\u{10ffff}")],
                |r| r.get(0),
            )?;
            if exists || last.is_some() {
                return Err(invalid());
            }
            0
        }
    };
    if previous > 0 && last.as_deref() != Some(format!("{GLOBAL}{previous:020}").as_str()) {
        return Err(invalid());
    }
    previous.checked_add(1).ok_or_else(invalid)
}
fn do_propose(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: AdminCaseCustodyPropose,
) -> Result<AdminCaseCustodyView> {
    if !matches!(by, Sender::Human) {
        return Err(invalid());
    }
    uuid(&p.request_id)?;
    let reason = text(&p.reason)?;
    let o = origin(tx.conn(), &p.crew_id, &p.case_id)?;
    let state = load_state(tx.conn(), &o)?;
    if let Some(t) = load_transfer(tx.conn(), &p.request_id)? {
        if t.view.case_id != p.case_id
            || t.view.crew_id != p.crew_id
            || t.proposal_revision != p.expected_revision
            || t.view.request_reason != reason
        {
            return Err(invalid());
        }
        return project(tx.conn(), &o, state.as_ref(), tx.now_ms());
    }
    let before = project(tx.conn(), &o, state.as_ref(), tx.now_ms())?;
    if before.revision != p.expected_revision || before.phase != Phase::Stale {
        return Err(invalid());
    }
    if before
        .pending
        .as_ref()
        .is_some_and(|v| v.phase != TransferPhase::Stale)
    {
        return Err(invalid());
    }
    let captain = crate::crews::captain_of(tx.conn(), &p.crew_id)?.ok_or_else(invalid)?;
    let authority = environment(tx.conn(), &p.crew_id, &captain)?;
    let targets = target_scope(tx.conn(), &o)?;
    let sequence = next_sequence(tx.conn())?;
    for id in [&before.current_custodian_session_id, &captain] {
        verify_recipient(tx.conn(), id)?;
    }
    let wake_id = match wakes::enqueue(
        tx,
        wakes::WakeRequest {
            session: captain.clone(),
            kind: wakes::WakeKind::System,
            source: format!("admin-case-custody:{}", p.request_id),
            priority: wakes::WakePriority::Auto,
            not_before_ms: None,
            note: None,
            origin: None,
        },
    )? {
        wakes::Enqueued::New(id) => id,
        _ => return Err(invalid()),
    };
    let mut t = Transfer {
        version: 1,
        sequence,
        authority,
        proposal_revision: p.expected_revision,
        accepted_revision: None,
        view: AdminCaseCustodyTransfer {
            request_id: p.request_id.clone(),
            case_id: p.case_id.clone(),
            crew_id: p.crew_id.clone(),
            predecessor_receipt_id: before.current_receipt_id,
            from_session_id: before.current_custodian_session_id,
            to_session_id: captain,
            phase: TransferPhase::Waiting,
            revision: uuid::Uuid::new_v4().to_string(),
            reason: Reason::Proposed,
            request_reason: reason,
            targets,
            wake_id,
            created_at: tx.now_ms(),
            delivered_at: None,
            accepted_at: None,
            acceptance_note: None,
            superseded_by: None,
        },
        checksum: String::new(),
    };
    let mut s = state.unwrap_or(State {
        version: 1,
        case: p.case_id,
        crew: p.crew_id,
        origin: o.request_id.clone(),
        original_digest: judgments::fingerprint(&o)?,
        first: None,
        current: None,
        pending: None,
        accepted_count: 0,
        revision: uuid::Uuid::new_v4().to_string(),
        discharge: None,
        checksum: String::new(),
    });
    if let Some(old) = &s.pending {
        let mut old = load_transfer(tx.conn(), old)?.ok_or_else(invalid)?;
        old.view.superseded_by = Some(p.request_id.clone());
        save_transfer(tx.conn(), &mut old)?;
    }
    save_transfer(tx.conn(), &mut t)?;
    meta_set(tx.conn(), COUNTER, &sequence.to_string())?;
    meta_set(tx.conn(), &format!("{GLOBAL}{sequence:020}"), &p.request_id)?;
    for id in [&t.view.from_session_id, &t.view.to_session_id] {
        meta_set(
            tx.conn(),
            &format!("{}{sequence:020}", recipient_prefix(id)?),
            &p.request_id,
        )?;
        save_recipient(tx.conn(), id)?;
    }
    s.pending = Some(p.request_id);
    s.revision = uuid::Uuid::new_v4().to_string();
    save_state(tx.conn(), &mut s)?;
    project(tx.conn(), &o, Some(&s), tx.now_ms())
}
#[derive(Clone, Debug)]
pub enum Operation {
    Get {
        crew_id: String,
        case_id: String,
    },
    List {
        after: Option<String>,
        limit: u32,
    },
    Accept {
        request_id: String,
        expected_revision: String,
        note: String,
    },
}
#[derive(Clone)]
pub struct ReadPermit {
    captain: String,
    crew: String,
    case: String,
}
impl ReadPermit {
    pub fn captain_session_id(&self) -> &str {
        &self.captain
    }
    pub fn crew_id(&self) -> &str {
        &self.crew
    }
    pub fn case_id(&self) -> &str {
        &self.case
    }
}
pub struct AcceptPermit {
    read: ReadPermit,
    request: String,
    revision: String,
    note: String,
    before_seq: u64,
}
impl AcceptPermit {
    pub fn captain_session_id(&self) -> &str {
        self.read.captain_session_id()
    }
    pub fn crew_id(&self) -> &str {
        self.read.crew_id()
    }
    pub fn case_id(&self) -> &str {
        self.read.case_id()
    }
}
fn accept_authority(
    c: &Connection,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
) -> Result<(Transfer, AdminCaseHandoffView, State)> {
    uuid(id)?;
    let note = text(note)?;
    let Sender::Session(actor) = by else {
        return Err(invalid());
    };
    let t = load_transfer(c, id)?.ok_or_else(invalid)?;
    let o = origin(c, &t.view.crew_id, &t.view.case_id)?;
    let s = load_state(c, &o)?.ok_or_else(invalid)?;
    if actor != &t.view.to_session_id || !transfer_current(c, &t, &o)? {
        return Err(invalid());
    }
    verify_recipient(c, &t.view.from_session_id)?;
    verify_recipient(c, &t.view.to_session_id)?;
    if t.view.phase == TransferPhase::Accepted {
        if s.current.as_deref() != Some(id)
            || t.accepted_revision.as_deref() != Some(revision)
            || t.view.acceptance_note.as_deref() != Some(&note)
        {
            return Err(invalid());
        }
    } else {
        let view = project_transfer(c, &t, &o)?;
        if s.pending.as_deref() != Some(id)
            || view.phase != TransferPhase::Delivered
            || view.revision != revision
        {
            return Err(invalid());
        }
    }
    Ok((t, o, s))
}
pub fn validate_operation(
    c: &Connection,
    by: &Sender,
    op: &Operation,
    now: i64,
) -> Result<ReadPermit> {
    let Sender::Session(caller) = by else {
        return Err(invalid());
    };
    let actor = crate::sessions::get(c, caller)?;
    crate::crews::refuse_archived(c, &actor.crew_id)?;
    match op {
        Operation::Get { crew_id, case_id } => {
            get(c, by, crew_id, case_id, now)?;
            Ok(ReadPermit {
                captain: caller.clone(),
                crew: crew_id.clone(),
                case: case_id.clone(),
            })
        }
        Operation::List { after, limit } => {
            list(c, by, after.clone(), *limit, now)?;
            Ok(ReadPermit {
                captain: caller.clone(),
                crew: actor.crew_id,
                case: String::new(),
            })
        }
        Operation::Accept {
            request_id,
            expected_revision,
            note,
        } => {
            let (t, _, _) = accept_authority(c, by, request_id, expected_revision, note)?;
            Ok(ReadPermit {
                captain: t.view.to_session_id,
                crew: t.view.crew_id,
                case: t.view.case_id,
            })
        }
    }
}
pub fn begin_accept(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
) -> Result<AcceptPermit> {
    let op = Operation::Accept {
        request_id: id.into(),
        expected_revision: revision.into(),
        note: note.into(),
    };
    Ok(AcceptPermit {
        read: validate_operation(tx.conn(), by, &op, tx.now_ms())?,
        request: id.into(),
        revision: revision.into(),
        note: text(note)?,
        before_seq: crate::asks::load_org(tx.conn())?.state().last_seq,
    })
}
pub fn finish_accept(tx: &mut Tx<'_>, p: AcceptPermit) -> Result<()> {
    if crate::asks::load_org(tx.conn())?.state().last_seq != p.before_seq {
        return Err(invalid());
    }
    let (t, _, _) = accept_authority(
        tx.conn(),
        &Sender::Session(p.read.captain),
        &p.request,
        &p.revision,
        &p.note,
    )?;
    if t.view.phase != TransferPhase::Accepted {
        return Err(invalid());
    }
    Ok(())
}
fn do_accept(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
) -> Result<AdminCaseCustodyTransfer> {
    let (mut t, o, mut s) = accept_authority(tx.conn(), by, id, revision, note)?;
    if t.view.phase == TransferPhase::Accepted {
        return project_transfer(tx.conn(), &t, &o);
    }
    t.view.delivered_at = project_transfer(tx.conn(), &t, &o)?.delivered_at;
    t.view.phase = TransferPhase::Accepted;
    t.view.reason = Reason::CaptainAccepted;
    t.view.accepted_at = Some(tx.now_ms());
    t.view.acceptance_note = Some(text(note)?);
    t.accepted_revision = Some(revision.into());
    t.view.revision = uuid::Uuid::new_v4().to_string();
    if let Some(old) = &s.current {
        let mut previous = load_transfer(tx.conn(), old)?.ok_or_else(invalid)?;
        if previous.view.superseded_by.is_some() {
            return Err(invalid());
        }
        previous.view.superseded_by = Some(id.into());
        save_transfer(tx.conn(), &mut previous)?;
    } else {
        s.first = Some(id.into());
    }
    save_transfer(tx.conn(), &mut t)?;
    s.current = Some(id.into());
    s.pending = None;
    s.accepted_count = s.accepted_count.checked_add(1).ok_or_else(invalid)?;
    s.revision = uuid::Uuid::new_v4().to_string();
    s.discharge = None;
    save_state(tx.conn(), &mut s)?;
    project_transfer(tx.conn(), &t, &o)
}
fn do_verify(tx: &mut Tx<'_>, crew: &str, case: &str) -> Result<AdminCaseCustodyView> {
    let o = origin(tx.conn(), crew, case)?;
    let Some(mut s) = load_state(tx.conn(), &o)? else {
        return project(tx.conn(), &o, None, tx.now_ms());
    };
    if let Some(id) = s.current.clone() {
        let t = load_transfer(tx.conn(), &id)?.ok_or_else(invalid)?;
        if transfer_current(tx.conn(), &t, &o)? {
            let (facts, fingerprint, complete) =
                business_facts(tx.conn(), crew, case, tx.now_ms())?;
            if complete && !discharge_valid(tx.conn(), &s, &o, tx.now_ms())? {
                let revision = judgments::fingerprint(&(&id, &t.authority, &fingerprint))?;
                s.discharge = Some(Discharge {
                    receipt: id,
                    authority: t.authority,
                    facts: fingerprint,
                    view: AdminCaseCustodyDischarge {
                        revision,
                        verified_at: tx.now_ms(),
                        targets: facts.targets,
                    },
                });
                s.revision = uuid::Uuid::new_v4().to_string();
                save_state(tx.conn(), &mut s)?;
            }
        }
    }
    project(tx.conn(), &o, Some(&s), tx.now_ms())
}
fn do_list(
    c: &Connection,
    by: &Sender,
    after: Option<String>,
    limit: u32,
    now: i64,
) -> Result<AdminCaseCustodyList> {
    let Sender::Session(id) = by else {
        return Err(invalid());
    };
    let actor = crate::sessions::get(c, id)?;
    crate::crews::refuse_archived(c, &actor.crew_id)?;
    verify_recipient(c, id)?;
    if limit == 0 || limit > 100 {
        return Err(invalid());
    }
    let cursor = judgments::fingerprint(&(id, crewd_store::judgment_state::epoch(c)?))?;
    let seq = match after {
        None => 0,
        Some(v) => {
            let (a, b) = v.rsplit_once(':').ok_or_else(invalid)?;
            let n = b.parse::<u64>().map_err(|_| invalid())?;
            if a != cursor || b != format!("{n:020}") {
                return Err(invalid());
            }
            n
        }
    };
    let prefix = recipient_prefix(id)?;
    let mut q =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let rows = q
        .query_map(
            params![
                format!("{prefix}{seq:020}"),
                format!("{prefix}\u{10ffff}"),
                i64::from(limit) + 1
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if actor.role != Role::Captain && recipient_digest(c, id)?.0 == 0 {
        return Err(invalid());
    }
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    let mut last = 0;
    for (index, request) in rows.into_iter().take(limit as usize) {
        let t = load_transfer(c, &request)?.ok_or_else(invalid)?;
        if t.view.crew_id != actor.crew_id
            || (t.view.from_session_id != *id && t.view.to_session_id != *id)
            || index != format!("{prefix}{:020}", t.sequence)
        {
            return Err(invalid());
        }
        let o = origin(c, &t.view.crew_id, &t.view.case_id)?;
        let _ = load_state(c, &o)?.ok_or_else(invalid)?;
        last = t.sequence;
        items.push(project_transfer(c, &t, &o)?);
    }
    let _ = now;
    Ok(AdminCaseCustodyList {
        items,
        next_after: more.then(|| format!("{cursor}:{last:020}")),
    })
}
pub(crate) fn original_obligation_released(c: &Connection, crew: &str, case: &str) -> Result<bool> {
    let o = origin(c, crew, case)?;
    Ok(load_state(c, &o)?.is_some_and(|s| s.accepted_count > 0))
}
pub(crate) fn has_unresolved_custody(c: &Connection, session: &str, now: i64) -> Result<bool> {
    verify_recipient(c, session)?;
    let prefix = recipient_prefix(session)?;
    let mut q = c.prepare("SELECT value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")?;
    let mut rows = q.query(params![prefix, format!("{prefix}\u{10ffff}")])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let t = load_transfer(c, &id)?.ok_or_else(invalid)?;
        if t.view.to_session_id == session && t.view.phase == TransferPhase::Accepted {
            let o = origin(c, &t.view.crew_id, &t.view.case_id)?;
            let s = load_state(c, &o)?.ok_or_else(invalid)?;
            if s.current.as_deref() == Some(&id) && !discharge_valid(c, &s, &o, now)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
pub(crate) fn render(
    c: &Connection,
    row: &wakes::WakeRow,
    _now: i64,
) -> Result<crate::work::FollowupNote> {
    let Some(id) = row.source.strip_prefix("admin-case-custody:") else {
        return Ok(crate::work::FollowupNote::Ordinary);
    };
    let Some(t) = load_transfer(c, id)? else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    let o = origin(c, &t.view.crew_id, &t.view.case_id)?;
    let s = load_state(c, &o)?.ok_or_else(invalid)?;
    if s.pending.as_deref() != Some(id)
        || t.view.phase != TransferPhase::Waiting
        || notification(c, &t)?.as_ref() != Some(row)
        || !transfer_current(c, &t, &o)?
    {
        return Ok(crate::work::FollowupNote::Stale);
    }
    let targets = t
        .view
        .targets
        .iter()
        .filter(|t| t.scope == AdminCaseScope::InScope)
        .map(|t| t.id.as_str())
        .take(50)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(crate::work::FollowupNote::Current(crate::prompts::fill(
        crate::prompts::render_file(crate::Locale::Zh).get("notice.admin.custody"),
        &[("case", &t.view.case_id), ("crew", &t.view.crew_id), ("request", id), ("targets", &targets), ("reason", &t.view.request_reason)],
    )))
}
pub fn propose(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: AdminCaseCustodyPropose,
) -> Result<AdminCaseCustodyView> {
    do_propose(tx, by, p)
}
pub fn get(
    c: &Connection,
    by: &Sender,
    crew: &str,
    case: &str,
    now: i64,
) -> Result<AdminCaseCustodyView> {
    let o = origin(c, crew, case)?;
    let s = load_state(c, &o)?;
    access(c, by, &o, s.as_ref())?;
    project(c, &o, s.as_ref(), now)
}
pub fn accept(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    revision: &str,
    note: &str,
) -> Result<AdminCaseCustodyTransfer> {
    do_accept(tx, by, id, revision, note)
}
/// Resolve an uncertain request by its original UUID, without issuing any mutation.
pub fn get_transfer(
    c: &Connection,
    by: &Sender,
    request_id: &str,
    _now: i64,
) -> Result<AdminCaseCustodyTransfer> {
    uuid(request_id)?;
    if matches!(by, Sender::System) {
        return Err(invalid());
    }
    let Some(t) = load_transfer(c, request_id)? else {
        let recorded: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key>=?1 AND key<?2 AND value=?3)",
            params![GLOBAL, format!("{GLOBAL}\u{10ffff}"), request_id],
            |row| row.get(0),
        )?;
        if recorded {
            return Err(
                StoreError::Invalid("missing administrative custody receipt".into()).into(),
            );
        }
        return Err(crate::CollabError::NotFound(
            crate::refusal::Refusal::new(crate::refusal::Code::ResourceNotFound)
                .arg("id", request_id),
        ));
    };
    if let Sender::Session(id) = by {
        let actor = crate::sessions::get(c, id)?;
        if actor.crew_id != t.view.crew_id
            || (id != &t.view.from_session_id
                && id != &t.view.to_session_id
                && !(actor.role == Role::Captain
                    && crate::crews::captain_of(c, &t.view.crew_id)?.as_ref() == Some(id)))
        {
            return Err(invalid());
        }
    }
    let o = origin(c, &t.view.crew_id, &t.view.case_id)?;
    let s = load_state(c, &o)?.ok_or_else(invalid)?;
    access(c, by, &o, Some(&s))?;
    project_transfer(c, &t, &o)
}
pub fn verify(tx: &mut Tx<'_>, crew: &str, case: &str) -> Result<AdminCaseCustodyView> {
    do_verify(tx, crew, case)
}
pub fn list(
    c: &Connection,
    by: &Sender,
    after: Option<String>,
    limit: u32,
    now: i64,
) -> Result<AdminCaseCustodyList> {
    do_list(c, by, after, limit, now)
}
pub fn status_for_case(
    c: &Connection,
    crew: &str,
    case: &str,
    now: i64,
) -> Result<Option<AdminCaseCustodyView>> {
    let Some(o) = handoff::custody_origin(c, crew, case)? else {
        return Ok(None);
    };
    let s = load_state(c, &o)?;
    Ok(Some(project(c, &o, s.as_ref(), now)?))
}
#[cfg(test)]
#[path = "admin_case_custody_review.rs"]
mod independent_review;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        admin_case_execution::tests::running_case,
        testkit::{NOW, ok, store},
    };
    fn setup() -> (crewd_store::Store, AdminCaseView) {
        setup_store(store())
    }
    fn setup_store(mut s: crewd_store::Store) -> (crewd_store::Store, AdminCaseView) {
        let (case, reg) = running_case(&mut s);
        let id = uuid::Uuid::new_v4().to_string();
        let request = ok(&mut s, |tx| {
            handoff::request(
                tx,
                &Sender::Session(reg.session.id),
                &id,
                "original custody",
            )
        });
        delivered(&mut s, request.wake_id, "cap-a");
        let v = handoff::get(s.conn(), &Sender::Session("cap-a".into()), &id, NOW).unwrap();
        ok(&mut s, |tx| {
            handoff::accept(
                tx,
                &Sender::Session("cap-a".into()),
                &id,
                &v.revision,
                "accepted",
            )
        });
        ok(&mut s, |tx| {
            crate::sessions::set_desired(tx, "w-a2", Desired::Running)?;
            crate::crews::set_captain(tx, "crew-a", "w-a2")
        });
        (s, case)
    }
    fn delivered(s: &mut crewd_store::Store, id: i64, cap: &str) {
        ok(s, |tx| {
            wakes::claim(tx, &[id])?;
            wakes::ack_delivered(tx, cap, &[id], wakes::Riders::Release, &[], &[])
        })
    }
    fn parameters(s: &crewd_store::Store, case: &AdminCaseView) -> AdminCaseCustodyPropose {
        AdminCaseCustodyPropose {
            crew_id: "crew-a".into(),
            case_id: case.id.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_revision: get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW)
                .unwrap()
                .revision,
            reason: "explicit replacement".into(),
        }
    }
    fn transfer(s: &mut crewd_store::Store, case: &AdminCaseView) -> AdminCaseCustodyTransfer {
        let p = parameters(s, case);
        let v = ok(s, |tx| propose(tx, &Sender::Human, p));
        let t = v.pending.unwrap();
        delivered(s, t.wake_id, &t.to_session_id);
        let t = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW)
            .unwrap()
            .pending
            .unwrap();
        ok(s, |tx| {
            accept(
                tx,
                &Sender::Session(t.to_session_id.clone()),
                &t.request_id,
                &t.revision,
                "accept",
            )
        })
    }
    #[test]
    fn uuid_retry_and_competing_proposals_do_not_replace_live_responsibility() {
        let (mut s, case) = setup();
        let p = parameters(&s, &case);
        let v = ok(&mut s, |tx| propose(tx, &Sender::Human, p.clone()));
        let retry = ok(&mut s, |tx| propose(tx, &Sender::Human, p.clone()));
        assert_eq!(v, retry);
        let mut changed = p.clone();
        changed.reason = "different body".into();
        assert!(crate::run(&mut s, NOW, |tx| propose(tx, &Sender::Human, changed)).is_err());
        let mut racing = p.clone();
        racing.request_id = uuid::Uuid::new_v4().to_string();
        assert!(crate::run(&mut s, NOW, |tx| propose(tx, &Sender::Human, racing)).is_err());
        assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        let replacement = parameters(&s, &case);
        let new = ok(&mut s, |tx| propose(tx, &Sender::Human, replacement));
        assert_ne!(new.pending.unwrap().wake_id, v.pending.unwrap().wake_id);
        let old = load_transfer(s.conn(), &p.request_id).unwrap().unwrap();
        assert!(old.view.superseded_by.is_some());
        assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
    }
    #[test]
    fn ordinary_file_close_and_reopen_preserves_pending_then_accepted_without_reissuing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custody.db");
        let file = crewd_store::Store::open(&path).unwrap();
        crewd_store::crews::upsert(
            file.conn(),
            &Crew {
                id: "crew-a".into(),
                title: "synthetic crew".into(),
                number: Some(1),
                working_directory: Some("/tmp/synthetic".into()),
                created_at: crewd_store::ts_from_ms(NOW),
                updated_at: crewd_store::ts_from_ms(NOW),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        for (id, role, ext) in [
            ("cap-a", Role::Captain, None),
            ("w-a1", Role::Worker, Some(2)),
            ("w-a2", Role::Worker, Some(3)),
        ] {
            crewd_store::sessions::upsert(
                file.conn(),
                &Session {
                    id: id.into(),
                    crew_id: "crew-a".into(),
                    role,
                    executor: "claude".into(),
                    ext,
                    updated_at: crewd_store::ts_from_ms(NOW),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        crewd_store::schema::backfill_v3(file.conn()).unwrap();
        crewd_store::schema::backfill_v4(file.conn()).unwrap();
        let (mut s, case) = setup_store(file);
        let p = parameters(&s, &case);
        let pending = ok(&mut s, |tx| propose(tx, &Sender::Human, p));
        let epoch = crewd_store::judgment_state::epoch(s.conn()).unwrap();
        let count: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM wakes", [], |r| r.get(0))
            .unwrap();
        s.finish().unwrap();
        drop(s);
        let mut reopened = crewd_store::Store::open(&path).unwrap();
        assert_eq!(
            crewd_store::judgment_state::epoch(reopened.conn()).unwrap(),
            epoch
        );
        assert_eq!(
            get(reopened.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap(),
            pending
        );
        assert_eq!(
            reopened
                .conn()
                .query_row("SELECT COUNT(*) FROM wakes", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
        let t = pending.pending.unwrap();
        delivered(&mut reopened, t.wake_id, &t.to_session_id);
        let t = get(reopened.conn(), &Sender::Human, "crew-a", &case.id, NOW)
            .unwrap()
            .pending
            .unwrap();
        ok(&mut reopened, |tx| {
            accept(
                tx,
                &Sender::Session(t.to_session_id.clone()),
                &t.request_id,
                &t.revision,
                "accepted after reopen",
            )
        });
        let accepted = get(reopened.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap();
        reopened.finish().unwrap();
        drop(reopened);
        let reopened = crewd_store::Store::open(&path).unwrap();
        assert_eq!(
            get(reopened.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap(),
            accepted
        );
        assert_eq!(
            list(
                reopened.conn(),
                &Sender::Session("w-a2".into()),
                None,
                64,
                NOW
            )
            .unwrap()
            .items
            .len(),
            1
        );
        assert!(handoff::has_unresolved_custody(reopened.conn(), "w-a2", NOW).unwrap());
        assert!(!handoff::has_unresolved_custody(reopened.conn(), "cap-a", NOW).unwrap());
        assert_eq!(
            reopened
                .conn()
                .query_row("SELECT COUNT(*) FROM wakes", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
        reopened.finish().unwrap();
    }
    #[test]
    fn historical_transfer_lookup_is_authorized_pure_and_distinguishes_absence_from_corruption() {
        let (mut s, case) = setup();
        let first = transfer(&mut s, &case);
        ok(&mut s, |tx| {
            crate::crews::set_captain(tx, "crew-a", "cap-a")
        });
        let second = transfer(&mut s, &case);
        let changes = s.conn().total_changes();
        let historical = get_transfer(s.conn(), &Sender::Human, &first.request_id, NOW).unwrap();
        assert_eq!(historical.request_id, first.request_id);
        assert_eq!(
            historical.superseded_by.as_deref(),
            Some(second.request_id.as_str())
        );
        assert!(
            get_transfer(
                s.conn(),
                &Sender::Session("w-a2".into()),
                &first.request_id,
                NOW
            )
            .is_ok()
        );
        assert!(
            get_transfer(
                s.conn(),
                &Sender::Session("cap-a".into()),
                &first.request_id,
                NOW
            )
            .is_ok()
        );
        assert!(
            get_transfer(
                s.conn(),
                &Sender::Session("w-a1".into()),
                &first.request_id,
                NOW
            )
            .is_err()
        );
        assert!(
            get_transfer(
                s.conn(),
                &Sender::Session("cap-b".into()),
                &first.request_id,
                NOW
            )
            .is_err()
        );
        assert!(matches!(
            get_transfer(
                s.conn(),
                &Sender::Human,
                &uuid::Uuid::new_v4().to_string(),
                NOW
            ),
            Err(crate::CollabError::NotFound(_))
        ));
        assert_eq!(
            s.conn().total_changes(),
            changes,
            "lookup must not create, retry, or reissue anything"
        );
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [key(&first.request_id)])
            .unwrap();
        let broken = get_transfer(s.conn(), &Sender::Human, &first.request_id, NOW);
        assert!(broken.is_err() && !matches!(broken, Err(crate::CollabError::NotFound(_))));
    }
    #[test]
    fn healthy_holder_cannot_repropose_but_accepted_request_retry_is_historical() {
        let (mut s, case) = setup();
        let p = parameters(&s, &case);
        let proposed = ok(&mut s, |tx| propose(tx, &Sender::Human, p.clone()));
        let t = proposed.pending.unwrap();
        delivered(&mut s, t.wake_id, &t.to_session_id);
        let t = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW)
            .unwrap()
            .pending
            .unwrap();
        ok(&mut s, |tx| {
            accept(
                tx,
                &Sender::Session(t.to_session_id.clone()),
                &t.request_id,
                &t.revision,
                "accepted",
            )
        });
        let current = ok(&mut s, |tx| propose(tx, &Sender::Human, p.clone()));
        assert_eq!(current.phase, Phase::Active);
        assert_eq!(current.current_receipt_id, p.request_id);
        let fresh = parameters(&s, &case);
        assert!(crate::run(&mut s, NOW, |tx| propose(tx, &Sender::Human, fresh)).is_err());
    }
    #[test]
    fn successor_chain_and_recipient_index_corruption_never_release_custody() {
        let (mut s, case) = setup();
        let first = transfer(&mut s, &case);
        assert!(!handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        assert!(handoff::has_unresolved_custody(s.conn(), "w-a2", NOW).unwrap());
        ok(&mut s, |tx| {
            crate::crews::set_captain(tx, "crew-a", "cap-a")
        });
        let second = transfer(&mut s, &case);
        assert!(!handoff::has_unresolved_custody(s.conn(), "w-a2", NOW).unwrap());
        assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
        let t = load_transfer(s.conn(), &second.request_id)
            .unwrap()
            .unwrap();
        let index = format!("{}{:020}", recipient_prefix("cap-a").unwrap(), t.sequence);
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [&index])
            .unwrap();
        assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).is_err());
        assert!(original_obligation_released(s.conn(), "crew-a", &case.id).is_err());
        meta_set(s.conn(), &index, &second.request_id).unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [key(&first.request_id)])
            .unwrap();
        assert!(get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW).is_err());
        assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).is_err());
    }
    #[test]
    fn recipient_pages_cover_more_than_sixty_four_receipts_and_restore_invalidates_cursor() {
        let (mut s, case) = setup();
        for _ in 0..65 {
            transfer(&mut s, &case);
            crewd_store::judgment_state::restored(s.conn()).unwrap();
        }
        let by = Sender::Session("w-a2".into());
        let first = list(s.conn(), &by, None, 64, NOW).unwrap();
        assert_eq!(first.items.len(), 64);
        let cursor = first.next_after.clone().unwrap();
        let last = list(s.conn(), &by, Some(cursor.clone()), 64, NOW).unwrap();
        assert_eq!(last.items.len(), 1);
        assert!(last.next_after.is_none());
        assert_ne!(first.items[63].request_id, last.items[0].request_id);
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(list(s.conn(), &by, Some(cursor), 64, NOW).is_err());
        assert!(handoff::has_unresolved_custody(s.conn(), "w-a2", NOW).unwrap());
    }
}
