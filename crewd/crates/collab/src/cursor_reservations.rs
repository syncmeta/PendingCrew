//! S44/D11 durable output ownership. Reservations are not read cursors: only confirmed
//! delivery/committed hook output advances a cursor. All row fates compose in the caller's Tx.
use crate::{CollabError, Result, Tx, cursors, wakes};
use crewd_proto::{Message, SenderKind};
use crewd_store::StoreError;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const PREFIX: &str = "cursor.reservation.v1.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Window {
    pub channel: String,
    pub from_exclusive: i64,
    pub to_inclusive: i64,
}

/// Caller cannot manufacture or modify a reservation receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    session: String,
    incarnation: String,
    token: String,
    executor_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    executor_generation: u64,
}

impl Owner {
    pub fn new(executor_generation: u64) -> Result<Self> {
        if executor_generation == 0 {
            return Err(invalid("zero executor generation"));
        }
        Ok(Self {
            executor_generation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Held,
    Cancelling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Active {
    token: String,
    executor_generation: u64,
    phase: Phase,
    windows: Vec<Window>,
    wake_ids: Vec<i64>,
    rider_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u32,
    session_id: String,
    incarnation: String,
    revision: u64,
    active: Option<Active>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    automatic_after: Option<i64>,
    session: String,
    // Automatic output eligibility, not process-running or retained-history read permission.
    reader_enabled: bool,
    state: Option<State>,
    bases: BTreeMap<String, Option<i64>>,
    prefixes: BTreeMap<String, Vec<Identity>>,
    eligible: std::collections::BTreeSet<String>,
    work_notes: BTreeMap<i64, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    id: String,
    seq: i64,
    sender: Option<String>,
    kind: SenderKind,
}

impl From<&Message> for Identity {
    fn from(m: &Message) -> Self {
        Self {
            id: m.id.clone(),
            seq: m.seq,
            sender: m.sender_session_id.clone(),
            kind: m.sender_kind,
        }
    }
}

impl Snapshot {
    /// Capture the exact current work text in the same DB job as rendering.
    pub fn capture_work_followups(
        &mut self,
        conn: &Connection,
        rows: &[wakes::WakeRow],
        now: i64,
    ) -> Result<()> {
        for row in rows {
            if row.session != self.session {
                return Err(invalid("work follow-up snapshot ownership mismatch"));
            }
            if let crate::work::FollowupNote::Current(note) =
                crate::work::followup_note(conn, row, now)?
            {
                self.work_notes.insert(row.id, note);
            }
        }
        Ok(())
    }

    pub(crate) fn automatic_after(&self,base:i64)->i64{base.max(self.automatic_after.unwrap_or(0))}
    pub(crate) fn causal_session(&self) -> &str {
        &self.session
    }
    /// Outer None means channel was not captured; Some(None) means captured without a cursor.
    pub fn cursor_base(&self, channel: &str) -> Option<Option<i64>> {
        self.bases.get(channel).copied()
    }

    pub fn blocks_channel(&self, channel: &str) -> bool {
        self.state
            .as_ref()
            .and_then(|s| s.active.as_ref())
            .is_some_and(|a| a.windows.iter().any(|w| w.channel == channel))
    }

    /// Construct only actual captured oldest-prefix windows; absence is never an invented zero.
    pub fn windows_for(&self, actual_targets: &[(String, i64)]) -> Result<Vec<Window>> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for (ch, end) in actual_targets {
            let Some(Some(base)) = self.cursor_base(ch) else {
                return Err(invalid("uncaptured or missing window cursor"));
            };
            if !seen.insert(ch)
                || !self.eligible.contains(ch)
                || *end <= base
                || !self
                    .prefixes
                    .get(ch)
                    .is_some_and(|ms| ms.iter().any(|m| m.seq == *end))
            {
                return Err(invalid("target is not a captured eligible prefix end"));
            }
            out.push(Window {
                channel: ch.clone(),
                from_exclusive: base,
                to_inclusive: *end,
            });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<T> {
    Applied(T),
    Stale,
    Busy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedUnread {
    pub snapshot: Snapshot,
    /// Actual unread messages, not rendered text; render remains the sole rendering implementation.
    pub blocks: Vec<(String, Vec<Message>)>,
}

fn invalid(reason: impl Into<String>) -> CollabError {
    StoreError::Invalid(reason.into()).into()
}

fn load(conn: &Connection, session: &str) -> Result<Option<State>> {
    if session.trim().is_empty() {
        return Err(invalid("empty reservation reader"));
    }
    match crewd_store::meta_get(conn, &format!("{PREFIX}{session}"))? {
        None => Ok(None),
        Some(raw) => {
            let state: State = serde_json::from_str(&raw)
                .map_err(|e| invalid(format!("invalid cursor reservation: {e}")))?;
            if state.version != 1
                || state.session_id != session
                || uuid::Uuid::parse_str(&state.incarnation).is_err()
                || state.revision == 0
            {
                return Err(invalid(
                    "invalid cursor reservation identity/version/revision",
                ));
            }
            if let Some(a) = &state.active {
                let mut seen = std::collections::BTreeSet::new();
                if uuid::Uuid::parse_str(&a.token).is_err()
                    || a.executor_generation == 0
                    || (a.windows.is_empty() && a.wake_ids.is_empty())
                    || a.windows.iter().any(|w| {
                        w.channel.trim().is_empty()
                            || w.from_exclusive < 0
                            || w.to_inclusive <= w.from_exclusive
                            || !seen.insert(w.channel.as_str())
                    })
                    || a.wake_ids.iter().chain(&a.rider_ids).any(|id| *id <= 0)
                {
                    return Err(invalid("invalid cursor reservation active window/receipt"));
                }
                let mut ids = std::collections::BTreeSet::new();
                if a.wake_ids
                    .iter()
                    .chain(&a.rider_ids)
                    .any(|id| !ids.insert(*id))
                {
                    return Err(invalid("duplicate cursor reservation wake/rider"));
                }
                let reader_exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                    [session],
                    |r| r.get(0),
                )?;
                if !reader_exists {
                    return Err(invalid("active reservation reader is missing"));
                }
                for w in &a.windows {
                    let channel_exists: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM channels WHERE id=?1)",
                        [&w.channel],
                        |r| r.get(0),
                    )?;
                    if !channel_exists
                        || cursors::cursor(conn, session, &w.channel)?
                            .is_none_or(|base| base < w.from_exclusive)
                    {
                        return Err(invalid(
                            "active reservation channel/cursor is missing or behind its base",
                        ));
                    }
                }
                let ids: Vec<i64> = a.wake_ids.iter().chain(&a.rider_ids).copied().collect();
                let rows = wakes::by_ids(conn, &ids)?;
                if rows.len() != ids.len() || rows.iter().any(|r| r.session != session) {
                    return Err(invalid("active reservation row ownership is invalid"));
                }
            }
            Ok(Some(state))
        }
    }
}

fn store_state(tx: &mut Tx<'_>, state: &State) -> Result<()> {
    crewd_store::meta_set(
        tx.conn(),
        &format!("{PREFIX}{}", state.session_id),
        &serde_json::to_string(state)?,
    )?;
    Ok(())
}

fn bump(state: &mut State) -> Result<()> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("cursor reservation revision overflow"))?;
    Ok(())
}

fn fresh(session: &str) -> State {
    State {
        version: 1,
        session_id: session.into(),
        incarnation: uuid::Uuid::new_v4().to_string(),
        revision: 0,
        active: None,
    }
}

/// Capture in the same database job/transaction as actual messages/rendering. Missing State is
/// compatible; missing cursor is observable but cannot manufacture a claimed window.
pub fn snapshot(
    conn: &Connection,
    session: &str,
    channels: &[String],
    now_ms: i64,
) -> Result<Snapshot> {
    snapshot_after(conn,session,channels,now_ms,None)
}
/// Trusted per-runtime rendering projection; never writes a read cursor or crosses IPC.
pub fn snapshot_after(conn:&Connection,session:&str,channels:&[String],now_ms:i64,automatic_after:Option<i64>)->Result<Snapshot>{
    if automatic_after.is_some_and(|n|n<0){return Err(invalid("negative automatic message floor"))}
    let state = load(conn, session)?;
    let reader = crate::sessions::get(conn, session)?;
    let reader_enabled = !crate::crews::is_archived(conn, &reader.crew_id)?;
    let surface = if reader_enabled {
        cursors::injection_channels(conn, session, now_ms)?
    } else {
        Vec::new()
    };
    let mut bases = BTreeMap::new();
    let mut prefixes = BTreeMap::new();
    let mut eligible = std::collections::BTreeSet::new();
    for channel in channels {
        if channel.trim().is_empty() {
            return Err(invalid("empty reservation channel"));
        }
        if bases.contains_key(channel) {
            return Err(invalid("duplicate snapshot channel"));
        }
        let base = cursors::cursor(conn, session, channel)?;
        bases.insert(channel.clone(), base);
        // Own-lobby membership intentionally includes retained archived history. New automatic
        // output acquisition has an additional live-channel requirement, rechecked at final CAS.
        let channel_enabled = conn
            .query_row(
                "SELECT archived_at IS NULL FROM channels WHERE id=?1",
                [channel],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if surface.contains(channel) && channel_enabled {
            eligible.insert(channel.clone());
            if let Some(base) = base {
                let (msgs, _) = cursors::scan_unread(conn, session, channel, base.max(automatic_after.unwrap_or(0)))?;
                prefixes.insert(channel.clone(), msgs.iter().map(Identity::from).collect());
            }
        }
    }
    Ok(Snapshot {
        automatic_after,
        session: session.into(),
        reader_enabled,
        state,
        bases,
        prefixes,
        eligible,
        work_notes: BTreeMap::new(),
    })
}

pub fn prepare_unread(tx: &mut Tx<'_>, session: &str) -> Result<PreparedUnread> {
    let blocks = cursors::automatic_unread_for(tx, session)?;
    let channels: Vec<String> = blocks.iter().map(|(ch, _)| ch.clone()).collect();
    Ok(PreparedUnread {
        snapshot: snapshot(tx.conn(), session, &channels, tx.now_ms())?,
        blocks,
    })
}

fn validate_prepared(tx: &Tx<'_>, prepared: &Snapshot, windows: &[Window]) -> Result<bool> {
    let channels: Vec<String> = prepared.bases.keys().cloned().collect();
    let current = snapshot_after(tx.conn(), &prepared.session, &channels, tx.now_ms(),prepared.automatic_after)?;
    if !current.reader_enabled
        || current.reader_enabled != prepared.reader_enabled
        || current.state != prepared.state
        || current.bases != prepared.bases
    {
        return Ok(false);
    }
    let mut seen = std::collections::BTreeSet::new();
    for w in windows {
        if !seen.insert(&w.channel) || w.from_exclusive < 0 || w.to_inclusive <= w.from_exclusive {
            return Err(invalid("invalid/duplicate reservation window"));
        }
        if current.cursor_base(&w.channel) != Some(Some(w.from_exclusive))
            || !current.eligible.contains(&w.channel)
            || current.blocks_channel(&w.channel)
        {
            return Ok(false);
        }
        let before = prepared
            .prefixes
            .get(&w.channel)
            .map(|ms| {
                ms.iter()
                    .filter(|m| m.seq <= w.to_inclusive)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let now = current
            .prefixes
            .get(&w.channel)
            .map(|ms| {
                ms.iter()
                    .filter(|m| m.seq <= w.to_inclusive)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if before.is_empty()
            || before.last().map(|m| m.seq) != Some(w.to_inclusive)
            || before != now
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn commit_hook(
    tx: &mut Tx<'_>,
    prepared: &Snapshot,
    actual_targets: &[(String, i64)],
) -> Result<Decision<()>> {
    let causes = crate::causal::capture(tx.conn(), prepared, actual_targets)?;
    Ok(
        match commit_hook_output(tx, prepared, &causes, actual_targets)? {
            Decision::Applied(_) => Decision::Applied(()),
            Decision::Stale => Decision::Stale,
            Decision::Busy => Decision::Busy,
        },
    )
}

fn commit_hook_inner(
    tx: &mut Tx<'_>,
    prepared: &Snapshot,
    actual_targets: &[(String, i64)],
) -> Result<Decision<()>> {
    let windows = prepared.windows_for(actual_targets)?;
    if !validate_prepared(tx, prepared, &windows)? {
        return Ok(Decision::Stale);
    }
    if windows.is_empty() {
        return Ok(Decision::Applied(()));
    }
    for w in &windows {
        cursors::advance(tx, &prepared.session, &w.channel, w.to_inclusive)?;
    }
    let mut state = load(tx.conn(), &prepared.session)?.unwrap_or_else(|| fresh(&prepared.session));
    bump(&mut state)?;
    store_state(tx, &state)?;
    Ok(Decision::Applied(()))
}

pub fn claim_reserved(
    tx: &mut Tx<'_>,
    prepared: &Snapshot,
    windows: &[Window],
    wake_ids: &[i64],
    rider_ids: &[i64],
    owner: Owner,
) -> Result<Decision<Lease>> {
    if windows.is_empty() && wake_ids.is_empty() {
        return Err(invalid("empty/rider-only reservation batch"));
    }
    if !validate_prepared(tx, prepared, windows)? {
        return Ok(Decision::Stale);
    }
    let mut state = load(tx.conn(), &prepared.session)?.unwrap_or_else(|| fresh(&prepared.session));
    if state.active.is_some() {
        return Ok(Decision::Busy);
    }
    let ids: Vec<i64> = wake_ids.iter().chain(rider_ids).copied().collect();
    let mut unique = std::collections::BTreeSet::new();
    if ids.iter().any(|id| *id <= 0 || !unique.insert(*id)) {
        return Err(invalid("invalid/duplicate wake IDs"));
    }
    let rows = wakes::by_ids(tx.conn(), &ids)?;
    if rows.len() != ids.len() || rows.iter().any(|r| r.session != prepared.session) {
        return Err(invalid("reservation wake ownership mismatch"));
    }
    // Ownership/terminal changes after rendering must not claim a stale body.
    // This is inside the same transaction that claims the queue and reservation.
    for row in rows.iter().filter(|r| wake_ids.contains(&r.id)) {
        match crate::work::followup_note(tx.conn(), row, tx.now_ms())? {
            crate::work::FollowupNote::Current(note)
                if prepared.work_notes.get(&row.id) == Some(&note) => {}
            crate::work::FollowupNote::Ordinary if !prepared.work_notes.contains_key(&row.id) => {}
            _ => return Ok(Decision::Stale),
        }
    }
    for row in rows
        .iter()
        .filter(|r| wake_ids.contains(&r.id) && r.kind == wakes::WakeKind::Message)
    {
        let Some(message_id) = row.source.strip_prefix("msg:") else {
            return Err(invalid("message wake source is invalid"));
        };
        let Some(message) = crewd_store::messages::get(tx.conn(), message_id)? else {
            return Ok(Decision::Stale);
        };
        if !windows.iter().any(|w| {
            w.channel == message.channel_id
                && message.seq > w.from_exclusive
                && message.seq <= w.to_inclusive
        }) {
            return Err(invalid(
                "body wake is outside the actual reserved message window",
            ));
        }
    }
    for row in rows.iter().filter(|r| wake_ids.contains(&r.id)) {
        crate::checkpoints::capture_for_claim(tx, row)?;
    }
    wakes::claim(tx, &ids)?;
    let token = uuid::Uuid::new_v4().to_string();
    state.active = Some(Active {
        token: token.clone(),
        executor_generation: owner.executor_generation,
        phase: Phase::Held,
        windows: windows.to_vec(),
        wake_ids: wake_ids.to_vec(),
        rider_ids: rider_ids.to_vec(),
    });
    bump(&mut state)?;
    store_state(tx, &state)?;
    Ok(Decision::Applied(Lease {
        session: prepared.session.clone(),
        incarnation: state.incarnation,
        token,
        executor_generation: owner.executor_generation,
    }))
}

fn exact(conn: &Connection, lease: &Lease) -> Result<Option<(State, Active)>> {
    let Some(state) = load(conn, &lease.session)? else {
        return Ok(None);
    };
    let Some(active) = state.active.clone() else {
        return Ok(None);
    };
    if state.incarnation != lease.incarnation
        || active.token != lease.token
        || active.executor_generation != lease.executor_generation
    {
        return Ok(None);
    }
    Ok(Some((state, active)))
}

fn clear(tx: &mut Tx<'_>, lease: &Lease) -> Result<()> {
    let Some((mut state, _)) = exact(tx.conn(), lease)? else {
        return Err(invalid("lease changed within transaction"));
    };
    state.active = None;
    bump(&mut state)?;
    store_state(tx, &state)
}

pub fn ack_reserved_delivered(
    tx: &mut Tx<'_>,
    lease: &Lease,
    riders: wakes::Riders<'_>,
) -> Result<Decision<()>> {
    if crate::causal::pending_for(
        tx.conn(),
        &lease.session,
        &lease.incarnation,
        &lease.token,
        lease.executor_generation,
    )?
    .is_some()
    {
        return Err(invalid("causal output requires exact successful witness"));
    }
    ack_reserved_delivered_inner(tx, lease, riders)
}

fn ack_reserved_delivered_inner(
    tx: &mut Tx<'_>,
    lease: &Lease,
    riders: wakes::Riders<'_>,
) -> Result<Decision<()>> {
    let Some((_, active)) = exact(tx.conn(), lease)? else {
        return Ok(Decision::Stale);
    };
    let targets = active
        .windows
        .iter()
        .map(|w| (w.channel.clone(), w.to_inclusive))
        .collect::<Vec<_>>();
    wakes::ack_delivered(
        tx,
        &lease.session,
        &active.wake_ids,
        riders,
        &active.rider_ids,
        &targets,
    )?;
    clear(tx, lease)?;
    Ok(Decision::Applied(()))
}

pub fn ack_reserved_failed(
    tx: &mut Tx<'_>,
    lease: &Lease,
    error: &str,
    decide: impl Fn(u32) -> Option<i64>,
) -> Result<Decision<Vec<wakes::Failed>>> {
    let Some((_, active)) = exact(tx.conn(), lease)? else {
        return Ok(Decision::Stale);
    };
    let ids = active
        .wake_ids
        .iter()
        .chain(&active.rider_ids)
        .copied()
        .collect::<Vec<_>>();
    let failed = wakes::ack_failed(tx, &ids, error, decide)?;
    if let Some(p) = crate::causal::pending_for(
        tx.conn(),
        &lease.session,
        &lease.incarnation,
        &lease.token,
        lease.executor_generation,
    )? {
        crate::causal::terminal(tx, &p)?;
    }
    clear(tx, lease)?;
    Ok(Decision::Applied(failed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal<'a> {
    Requeue,
    Dropped(&'a str),
}

pub fn settle_reserved(
    tx: &mut Tx<'_>,
    lease: &Lease,
    terminal: Terminal<'_>,
) -> Result<Decision<()>> {
    let Some((_, active)) = exact(tx.conn(), lease)? else {
        return Ok(Decision::Stale);
    };
    let ids = active
        .wake_ids
        .iter()
        .chain(&active.rider_ids)
        .copied()
        .collect::<Vec<_>>();
    match terminal {
        Terminal::Requeue => wakes::requeue(tx, &ids)?,
        Terminal::Dropped(error) => {
            for id in ids {
                wakes::mark_dropped(tx, id, error)?;
            }
        }
    }
    if let Some(p) = crate::causal::pending_for(
        tx.conn(),
        &lease.session,
        &lease.incarnation,
        &lease.token,
        lease.executor_generation,
    )? {
        crate::causal::terminal(tx, &p)?;
    }
    clear(tx, lease)?;
    Ok(Decision::Applied(()))
}

/// Release without cursor advance; exact receipt must survive release/reacquire ABA.
/// This scaffold's ABA contract uses zero wake IDs. Production must reject release while any
/// owned wake/rider is still inflight: row fate and opening the window belong in the same Tx.
pub fn release_reserved(tx: &mut Tx<'_>, lease: &Lease) -> Result<Decision<()>> {
    let Some((_, active)) = exact(tx.conn(), lease)? else {
        return Ok(Decision::Stale);
    };
    let ids = active
        .wake_ids
        .iter()
        .chain(&active.rider_ids)
        .copied()
        .collect::<Vec<_>>();
    let rows = wakes::by_ids(tx.conn(), &ids)?;
    if rows.len() != ids.len() || rows.iter().any(|r| r.state == wakes::WakeState::Inflight) {
        return Err(invalid("release before owned row fate"));
    }
    if let Some(p) = crate::causal::pending_for(
        tx.conn(),
        &lease.session,
        &lease.incarnation,
        &lease.token,
        lease.executor_generation,
    )? {
        crate::causal::terminal(tx, &p)?;
    }
    clear(tx, lease)?;
    Ok(Decision::Applied(()))
}

fn all_states(conn: &Connection) -> Result<Vec<State>> {
    let mut statement =
        conn.prepare("SELECT key FROM meta WHERE substr(key,1,?1)=?2 ORDER BY key")?;
    let keys = statement
        .query_map(params![PREFIX.len() as i64, PREFIX], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    keys.into_iter()
        .map(|key| {
            load(conn, &key[PREFIX.len()..])?.ok_or_else(|| invalid("reservation disappeared"))
        })
        .collect()
}

pub fn reset_inflight_and_reservations(
    tx: &mut Tx<'_>,
    error: &str,
    decide: impl Fn(u32) -> Option<i64>,
) -> Result<Vec<wakes::Failed>> {
    let states = all_states(tx.conn())?;
    guard_prune(tx.conn())?;
    let failed = wakes::reset_inflight(tx, error, decide)?;
    crate::causal::reset_pending(tx)?;
    for mut state in states {
        if state.active.take().is_some() {
            bump(&mut state)?;
            store_state(tx, &state)?;
        }
    }
    Ok(failed)
}

pub(crate) fn held_channels(conn: &Connection, session: &str) -> Result<Vec<String>> {
    Ok(load(conn, session)?
        .and_then(|s| s.active)
        .map(|a| a.windows.into_iter().map(|w| w.channel).collect())
        .unwrap_or_default())
}

pub(crate) fn validate_reader(conn: &Connection, session: &str) -> Result<()> {
    if session != cursors::HUMAN {
        load(conn, session)?;
    }
    Ok(())
}

pub(crate) fn channel_held(conn: &Connection, session: &str, channel: &str) -> Result<bool> {
    if session == cursors::HUMAN {
        return Ok(false);
    }
    Ok(held_channels(conn, session)?.iter().any(|ch| ch == channel))
}

/// Bare explicit reads retain their existing proved displayed-prefix rule. They invalidate
/// prepared bases/epochs but never steal the active receipt or open its window.
pub(crate) fn cursor_changed(tx: &mut Tx<'_>, session: &str) -> Result<()> {
    if session == cursors::HUMAN {
        return Ok(());
    }
    if let Some(mut state) = load(tx.conn(), session)? {
        bump(&mut state)?;
        store_state(tx, &state)?;
    }
    Ok(())
}

pub(crate) fn cancel_active(tx: &mut Tx<'_>, session: &str) -> Result<()> {
    if let Some(mut state) = load(tx.conn(), session)?
        && let Some(active) = &mut state.active
        && active.phase != Phase::Cancelling
    {
        active.phase = Phase::Cancelling;
        bump(&mut state)?;
        store_state(tx, &state)?;
    }
    Ok(())
}

pub(crate) fn guard_crew(conn: &Connection, crew: &str) -> Result<()> {
    let mut count = 0;
    for state in all_states(conn)? {
        let Some(active) = state.active else {
            continue;
        };
        let reader_crew: Option<String> = conn
            .query_row(
                "SELECT crew_id FROM sessions WHERE id=?1",
                [&state.session_id],
                |r| r.get(0),
            )
            .optional()?;
        let mut conflict = reader_crew.as_deref() == Some(crew);
        for window in active.windows {
            let channel_crew: Option<String> = conn
                .query_row(
                    "SELECT crew_id FROM channels WHERE id=?1",
                    [&window.channel],
                    |r| r.get(0),
                )
                .optional()?;
            if channel_crew.as_deref() == Some(crew) {
                conflict = true;
            }
        }
        if conflict {
            count += 1;
        }
    }
    if count > 0 {
        return Err(CollabError::Conflict(
            crate::refusal::Refusal::new(crate::refusal::Code::CrewWakesInflight)
                .arg("crew", crew)
                .arg("count", count.to_string()),
        ));
    }
    Ok(())
}

pub(crate) fn prune_orphan_readers(tx: &mut Tx<'_>) -> Result<()> {
    let states = all_states(tx.conn())?;
    for state in states {
        let exists: bool = tx.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            [&state.session_id],
            |r| r.get(0),
        )?;
        if !exists {
            tx.conn().execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("{PREFIX}{}", state.session_id)],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn guard_prune(conn: &Connection) -> Result<()> {
    for state in all_states(conn)? {
        if let Some(active) = state.active {
            let ids = active
                .wake_ids
                .iter()
                .chain(&active.rider_ids)
                .copied()
                .collect::<Vec<_>>();
            let rows = wakes::by_ids(conn, &ids)?;
            if rows.len() != ids.len()
                || rows
                    .iter()
                    .any(|r| r.session != state.session_id || r.state != wakes::WakeState::Inflight)
            {
                return Err(invalid(
                    "active reservation row was lost/settled before release",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCausalUnread {
    pub snapshot: Snapshot,
    pub blocks: Vec<(String, Vec<Message>)>,
    pub causes: crate::causal::CausalSnapshot,
}
pub fn prepare_causal_unread(tx: &mut Tx<'_>, session: &str) -> Result<PreparedCausalUnread> {
    prepare_causal_unread_after(tx,session,None)
}
pub fn prepare_causal_unread_after(tx:&mut Tx<'_>,session:&str,after:Option<i64>)->Result<PreparedCausalUnread>{
    let mut prepared = prepare_unread(tx, session)?;
    if let Some(after)=after {
        if after<0{return Err(invalid("negative automatic message floor"))}
        let mut blocks=Vec::new();
        for channel in cursors::injection_channels(tx.conn(),session,tx.now_ms())? {
            if let Some(base)=cursors::cursor(tx.conn(),session,&channel)? {
                let (messages,_)=cursors::scan_unread(tx.conn(),session,&channel,base.max(after))?;
                if !messages.is_empty(){blocks.push((channel,messages));}
            }
        }
        let channels:Vec<_>=blocks.iter().map(|(ch,_)|ch.clone()).collect();
        prepared.snapshot=snapshot_after(tx.conn(),session,&channels,tx.now_ms(),Some(after))?;
        prepared.blocks=blocks;
    }
    let ends = prepared
        .blocks
        .iter()
        .filter_map(|(ch, ms)| ms.last().map(|m| (ch.clone(), m.seq)))
        .collect::<Vec<_>>();
    let causes = crate::causal::capture(tx.conn(), &prepared.snapshot, &ends)?;
    Ok(PreparedCausalUnread {
        snapshot: prepared.snapshot,
        blocks: prepared.blocks,
        causes,
    })
}
pub fn claim_output(
    tx: &mut Tx<'_>,
    prepared: &Snapshot,
    causes: &crate::causal::CausalSnapshot,
    windows: &[Window],
    ids: &[i64],
    riders: &[i64],
    owner: Owner,
) -> Result<Decision<crate::causal::ClaimedOutput>> {
    if causes.unresolved() {
        return Ok(Decision::Busy);
    }
    if !causes.matches_windows(tx.conn(), prepared, windows)? {
        return Ok(Decision::Stale);
    }
    if crate::causal::inspect_admission(
        tx.conn(),
        causes,
        crate::causal::CAUSAL_HOP_MAX,
        tx.now_ms(),
    )?
    .blocked()
    {
        return Ok(Decision::Busy);
    }
    match claim_reserved(tx, prepared, windows, ids, riders, owner)? {
        Decision::Applied(lease) => {
            let pending = crate::causal::register_pending(
                tx,
                &lease.session,
                &lease.incarnation,
                &lease.token,
                lease.executor_generation,
                causes,
                ids,
            )?;
            Ok(Decision::Applied(crate::causal::ClaimedOutput {
                lease,
                pending,
            }))
        }
        Decision::Stale => Ok(Decision::Stale),
        Decision::Busy => Ok(Decision::Busy),
    }
}
/// A missing success witness may only be ignored for an already retired,
/// receipt. It remains Stale even after an earlier witnessed commit: an unwitnessed
/// replay must not report a new Applied. Keep owner inspection inside collab.
pub fn output_receipt_retired(tx: &Tx<'_>, claimed: &crate::causal::ClaimedOutput) -> Result<bool> {
    let current = crate::causal::pending_for(
        tx.conn(),
        &claimed.lease.session,
        &claimed.lease.incarnation,
        &claimed.lease.token,
        claimed.lease.executor_generation,
    )?;
    Ok(current.as_ref() != Some(&claimed.pending))
}
pub fn ack_output_delivered(
    tx: &mut Tx<'_>,
    claimed: &crate::causal::ClaimedOutput,
    success: &crate::causal::ObservedSuccess,
    riders: wakes::Riders<'_>,
) -> Result<Decision<crate::causal::OutputCommit>> {
    if success.pending() != claimed.pending {
        return Err(invalid("successful output does not own claimed receipt"));
    }
    if crate::causal::committed(tx.conn(), &claimed.pending)? {
        return Ok(Decision::Applied(crate::causal::finalize(
            tx,
            &claimed.pending,
            success,
        )?));
    }
    let p = crate::causal::pending_for(
        tx.conn(),
        &claimed.lease.session,
        &claimed.lease.incarnation,
        &claimed.lease.token,
        claimed.lease.executor_generation,
    )?;
    if p.as_ref() != Some(&claimed.pending) {
        return Ok(Decision::Stale);
    }
    match ack_reserved_delivered_inner(tx, &claimed.lease, riders)? {
        Decision::Applied(()) => Ok(Decision::Applied(crate::causal::finalize(
            tx,
            &claimed.pending,
            success,
        )?)),
        Decision::Stale => Ok(Decision::Stale),
        Decision::Busy => Ok(Decision::Busy),
    }
}
pub fn ack_output_failed(
    tx: &mut Tx<'_>,
    claimed: &crate::causal::ClaimedOutput,
    error: &str,
    decide: impl Fn(u32) -> Option<i64>,
) -> Result<Decision<Vec<wakes::Failed>>> {
    if crate::causal::pending_for(
        tx.conn(),
        &claimed.lease.session,
        &claimed.lease.incarnation,
        &claimed.lease.token,
        claimed.lease.executor_generation,
    )?
    .as_ref()
        != Some(&claimed.pending)
    {
        return Ok(Decision::Stale);
    }
    ack_reserved_failed(tx, &claimed.lease, error, decide)
}
pub fn settle_output(
    tx: &mut Tx<'_>,
    claimed: &crate::causal::ClaimedOutput,
    fate: Terminal<'_>,
) -> Result<Decision<()>> {
    if crate::causal::pending_for(
        tx.conn(),
        &claimed.lease.session,
        &claimed.lease.incarnation,
        &claimed.lease.token,
        claimed.lease.executor_generation,
    )?
    .as_ref()
        != Some(&claimed.pending)
    {
        return Ok(Decision::Stale);
    }
    settle_reserved(tx, &claimed.lease, fate)
}
pub fn commit_hook_output(
    tx: &mut Tx<'_>,
    prepared: &Snapshot,
    causes: &crate::causal::CausalSnapshot,
    targets: &[(String, i64)],
) -> Result<Decision<crate::causal::HookCommit>> {
    let windows = prepared.windows_for(targets)?;
    if causes.unresolved() {
        return Ok(Decision::Busy);
    }
    if !causes.matches_windows(tx.conn(), prepared, &windows)? {
        return Ok(Decision::Stale);
    }
    if crate::causal::inspect_admission(
        tx.conn(),
        causes,
        crate::causal::CAUSAL_HOP_MAX,
        tx.now_ms(),
    )?
    .blocked()
    {
        return Ok(Decision::Busy);
    }
    match commit_hook_inner(tx, prepared, targets)? {
        Decision::Applied(()) => {
            if windows.is_empty() {
                return Ok(Decision::Applied(crate::causal::HookCommit {
                    output: None,
                }));
            }
            let pending = crate::causal::hook_pending(tx, &prepared.session, causes)?;
            crate::causal::finalize_hook(tx, &pending)?;
            Ok(Decision::Applied(crate::causal::HookCommit {
                output: Some(pending.stable_key().into()),
            }))
        }
        Decision::Stale => Ok(Decision::Stale),
        Decision::Busy => Ok(Decision::Busy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, msg, ok, store};
    use crewd_proto::SenderKind;
    use crewd_store::Store;
    use rusqlite::params;

    const SESSION: &str = "w-a1";
    const CHANNEL: &str = "crew-a/lobby";

    fn fixture() -> (Store, Snapshot, Window) {
        let s = store();
        let end = msg(
            &s,
            CHANNEL,
            "reserved-body",
            SenderKind::Session,
            Some("cap-a"),
            vec![],
        );
        let snap = snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW).unwrap();
        let base = snap.cursor_base(CHANNEL).unwrap().unwrap();
        (
            s,
            snap,
            Window {
                channel: CHANNEL.into(),
                from_exclusive: base,
                to_inclusive: end,
            },
        )
    }

    fn applied<T: std::fmt::Debug>(decision: Decision<T>) -> T {
        match decision {
            Decision::Applied(v) => v,
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    fn claim(s: &mut Store, snap: &Snapshot, window: &Window, ids: &[i64]) -> Lease {
        applied(ok(s, |tx| {
            claim_reserved(
                tx,
                snap,
                std::slice::from_ref(window),
                ids,
                &[],
                Owner::new(1)?,
            )
        }))
    }

    fn wake(s: &mut Store) -> i64 {
        match ok(s, |tx| {
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: SESSION.into(),
                    kind: wakes::WakeKind::Message,
                    source: wakes::source::message("reserved-body"),
                    priority: wakes::WakePriority::Auto,
                    not_before_ms: None,
                    note: None,
                    origin: Some("cap-a".into()),
                },
            )
        }) {
            wakes::Enqueued::New(id) => id,
            other => panic!("unexpected fixture wake: {other:?}"),
        }
    }

    type DurableRows = (
        Vec<(String, String)>,
        Vec<(String, String, i64, String)>,
        Vec<(i64, String, i64)>,
    );

    fn durable_rows(s: &Store) -> DurableRows {
        let meta = s
            .conn()
            .prepare("SELECT key,value FROM meta ORDER BY key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let cursors = s.conn().prepare("SELECT reader,channel_id,seq,updated_at FROM read_cursors ORDER BY reader,channel_id").unwrap()
            .query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let wakes = s
            .conn()
            .prepare("SELECT id,state,attempts FROM wakes ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        (meta, cursors, wakes)
    }

    #[test]
    fn snapshot_absence_distinguishes_uncaptured_and_missing_cursor() {
        let s = store();
        let snap = snapshot(
            s.conn(),
            SESSION,
            &[CHANNEL.into(), "synthetic-missing".into()],
            NOW,
        )
        .unwrap();
        assert_eq!(snap.cursor_base(CHANNEL), Some(Some(0)));
        assert_eq!(snap.cursor_base("synthetic-missing"), Some(None));
        assert_eq!(snap.cursor_base("not-captured"), None);
        assert!(!snap.blocks_channel(CHANNEL));
    }

    #[test]
    fn snapshot_present_malformed_is_invalid_not_absent() {
        let s = store();
        s.conn()
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)",
                params![format!("{PREFIX}{SESSION}"), "{broken"],
            )
            .unwrap();
        assert!(matches!(
            snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW),
            Err(CollabError::Store(StoreError::Invalid(_)))
        ));
    }

    #[test]
    fn claim_and_reservation_insert_abort_roll_back_wake_attempt_cursor_and_meta() {
        let (mut s, snap, window) = fixture();
        let id = wake(&mut s);
        let before = durable_rows(&s);
        s.conn().execute_batch("CREATE TEMP TRIGGER reservation_insert_abort BEFORE INSERT ON meta WHEN substr(NEW.key,1,22)='cursor.reservation.v1.' BEGIN SELECT RAISE(ABORT,'synthetic reservation insert abort'); END;").unwrap();
        let error = crate::run(&mut s, NOW, |tx| {
            claim_reserved(tx, &snap, &[window], &[id], &[], Owner::new(1)?)
        })
        .unwrap_err();
        // An arbitrary Invalid/pending refusal is not evidence that the SQLite trigger fired.
        assert!(
            matches!(&error, CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code, Some(message))))
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
                && message == "synthetic reservation insert abort"),
            "expected actual INSERT trigger abort, got {error}"
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn zero_wake_ids_reservation_survives_database_reopen() {
        let (mut s, snap, window) = fixture();
        let _lease = claim(&mut s, &snap, &window, &[]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.sqlite");
        s.conn()
            .execute("VACUUM INTO ?1", params![path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let reopened = Store::open(&path).unwrap();
        assert!(
            snapshot(reopened.conn(), SESSION, &[CHANNEL.into()], NOW)
                .unwrap()
                .blocks_channel(CHANNEL)
        );
        assert_eq!(
            crate::testkit::count(&reopened, "SELECT COUNT(*) FROM wakes"),
            0
        );
        assert_eq!(
            cursors::cursor(reopened.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.from_exclusive)
        );
    }

    #[test]
    fn hook_commit_wins_then_prepared_wake_is_stale_without_claiming() {
        let (mut s, snap, window) = fixture();
        let id = wake(&mut s);
        applied(ok(&mut s, |tx| {
            commit_hook(tx, &snap, &[(CHANNEL.into(), window.to_inclusive)])
        }));
        let before = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &snap,
                &[window],
                &[id],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn wake_claim_wins_then_prepared_hook_is_stale_without_advancing() {
        let (mut s, snap, window) = fixture();
        let id = wake(&mut s);
        let _lease = claim(&mut s, &snap, &window, &[id]);
        let before = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| commit_hook(
                tx,
                &snap,
                &[(CHANNEL.into(), window.to_inclusive)]
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn released_epoch_does_not_allow_old_snapshot_or_receipt_to_touch_new_lease() {
        let (mut s, snap, window) = fixture();
        let old = claim(&mut s, &snap, &window, &[]);
        applied(ok(&mut s, |tx| release_reserved(tx, &old)));
        let fresh = snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW).unwrap();
        assert_ne!(fresh, snap, "release must retain a changed durable epoch");
        let current = claim(&mut s, &fresh, &window, &[]);
        assert_ne!(current, old);
        let before = durable_rows(&s);
        assert_eq!(ok(&mut s, |tx| release_reserved(tx, &old)), Decision::Stale);
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &snap,
                std::slice::from_ref(&window),
                &[],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn active_frontier_blocks_suffix_without_advancing_cursor() {
        let (mut s, snap, window) = fixture();
        let _lease = claim(&mut s, &snap, &window, &[]);
        msg(
            &s,
            CHANNEL,
            "own-behind-hole",
            SenderKind::Session,
            Some(SESSION),
            vec![],
        );
        msg(
            &s,
            CHANNEL,
            "suffix-behind-hole",
            SenderKind::Session,
            Some("cap-a"),
            vec![],
        );
        let before = durable_rows(&s);
        let prepared = ok(&mut s, |tx| prepare_unread(tx, SESSION));
        assert!(
            !prepared
                .blocks
                .iter()
                .any(|(channel, _)| channel == CHANNEL)
        );
        assert_eq!(durable_rows(&s), before);
    }

    fn sql_abort(error: &CollabError, sentinel: &str) {
        assert!(
            matches!(error, CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code,Some(message))))
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == sentinel),
            "actual SQL abort required: {error}"
        );
    }

    #[test]
    fn actual_generation_and_timer_only_empty_window_are_valid() {
        let mut s = store();
        assert!(Owner::new(0).is_err());
        let id = match ok(&mut s, |tx| {
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: SESSION.into(),
                    kind: wakes::WakeKind::Timer,
                    source: wakes::source::timer("synthetic-timer"),
                    priority: wakes::WakePriority::System,
                    not_before_ms: None,
                    note: Some("timer-only body".into()),
                    origin: None,
                },
            )
        }) {
            wakes::Enqueued::New(id) => id,
            other => panic!("{other:?}"),
        };
        let prepared = snapshot(s.conn(), SESSION, &[], NOW).unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| claim_reserved(
                tx,
                &prepared,
                &[],
                &[],
                &[],
                Owner::new(7)?
            ))
            .is_err()
        );
        assert!(
            crate::run(&mut s, NOW, |tx| claim_reserved(
                tx,
                &prepared,
                &[],
                &[],
                &[id],
                Owner::new(7)?
            ))
            .is_err()
        );
        let lease = applied(ok(&mut s, |tx| {
            claim_reserved(tx, &prepared, &[], &[id], &[], Owner::new(7)?)
        }));
        assert_eq!(lease.executor_generation, 7);
        // Parser must accept this actual persisted record, not treat no message windows as corruption.
        assert!(load(s.conn(), SESSION).unwrap().unwrap().active.is_some());
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        }));
        assert_eq!(
            wakes::by_ids(s.conn(), &[id]).unwrap()[0].state,
            wakes::WakeState::Delivered
        );
    }

    #[test]
    fn windows_require_real_captured_prefix_and_exact_base() {
        let (s, prepared, window) = fixture();
        assert_eq!(
            prepared
                .windows_for(&[(CHANNEL.into(), window.to_inclusive)])
                .unwrap(),
            vec![window]
        );
        assert!(prepared.windows_for(&[(CHANNEL.into(), 99999)]).is_err());
        let missing = snapshot(s.conn(), SESSION, &["synthetic-missing".into()], NOW).unwrap();
        assert!(
            missing
                .windows_for(&[("synthetic-missing".into(), 1)])
                .is_err()
        );
    }

    #[test]
    fn new_tail_does_not_expand_the_reserved_or_delivered_prefix() {
        let (mut s, prepared, window) = fixture();
        let tail = msg(
            &s,
            CHANNEL,
            "new-tail",
            SenderKind::Session,
            Some("cap-a"),
            vec![],
        );
        let lease = claim(&mut s, &prepared, &window, &[]);
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        }));
        assert_eq!(
            cursors::cursor(s.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.to_inclusive)
        );
        let next = ok(&mut s, |tx| prepare_unread(tx, SESSION));
        assert_eq!(
            next.blocks
                .iter()
                .find(|(ch, _)| ch == CHANNEL)
                .unwrap()
                .1
                .iter()
                .map(|m| m.seq)
                .collect::<Vec<_>>(),
            vec![tail]
        );
    }

    #[test]
    fn removed_prepared_message_is_stale_without_writing() {
        let (mut s, prepared, window) = fixture();
        s.conn()
            .execute("DELETE FROM messages WHERE id='reserved-body'", [])
            .unwrap();
        let before = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &prepared,
                &[window],
                &[],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn expired_listener_preparation_is_stale_at_commit_time() {
        let mut s = store();
        let reader = crate::testkit::org_worker(&mut s);
        ok(&mut s, |tx| {
            crate::listens::set(tx, &reader, CHANNEL, Some(NOW + 10), vec![])
        });
        let end = msg(
            &s,
            CHANNEL,
            "listen-body",
            SenderKind::Session,
            Some("cap-a"),
            vec![],
        );
        let prepared = snapshot(s.conn(), &reader, &[CHANNEL.into()], NOW).unwrap();
        let windows = prepared.windows_for(&[(CHANNEL.into(), end)]).unwrap();
        let before = durable_rows(&s);
        assert_eq!(
            crate::run(&mut s, NOW + 11, |tx| claim_reserved(
                tx,
                &prepared,
                &windows,
                &[],
                &[],
                Owner::new(1)?
            ))
            .unwrap()
            .0,
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn delivered_meta_update_abort_rolls_back_rows_cursor_and_exact_lease() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let lease = claim(&mut s, &prepared, &window, &[id]);
        let before = durable_rows(&s);
        let rows = wakes::by_ids(s.conn(), &[id]).unwrap();
        s.conn().execute_batch("CREATE TEMP TRIGGER reservation_update_abort BEFORE UPDATE ON meta WHEN substr(NEW.key,1,22)='cursor.reservation.v1.' BEGIN SELECT RAISE(ABORT,'synthetic reservation update abort'); END;").unwrap();
        let error = crate::run(&mut s, NOW, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        })
        .unwrap_err();
        sql_abort(&error, "synthetic reservation update abort");
        assert_eq!(durable_rows(&s), before);
        assert_eq!(wakes::by_ids(s.conn(), &[id]).unwrap(), rows);
        s.conn()
            .execute_batch("DROP TRIGGER reservation_update_abort")
            .unwrap();
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        }));
        assert!(
            !snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW)
                .unwrap()
                .blocks_channel(CHANNEL)
        );
    }

    #[test]
    fn delivered_cursor_abort_rolls_back_row_confirmation_and_release() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let lease = claim(&mut s, &prepared, &window, &[id]);
        let before = durable_rows(&s);
        s.conn().execute_batch("CREATE TEMP TRIGGER cursor_update_abort BEFORE UPDATE ON read_cursors BEGIN SELECT RAISE(ABORT,'synthetic cursor update abort'); END;").unwrap();
        let error = crate::run(&mut s, NOW, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        })
        .unwrap_err();
        sql_abort(&error, "synthetic cursor update abort");
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn failed_policy_uses_stored_attempt_and_releases_without_advancing() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let lease = claim(&mut s, &prepared, &window, &[id]);
        let failed = applied(ok(&mut s, |tx| {
            ack_reserved_failed(tx, &lease, "synthetic failure", |n| {
                assert_eq!(n, 1);
                Some(NOW + 55)
            })
        }));
        assert_eq!(failed[0].retry_at_ms, Some(NOW + 55));
        let row = &wakes::by_ids(s.conn(), &[id]).unwrap()[0];
        assert_eq!(row.state, wakes::WakeState::Pending);
        assert_eq!(row.attempts, 1);
        assert_eq!(row.last_error.as_deref(), Some("synthetic failure"));
        assert_eq!(
            cursors::cursor(s.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.from_exclusive)
        );
        assert!(
            !snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW)
                .unwrap()
                .blocks_channel(CHANNEL)
        );
        assert!(
            !ok(&mut s, |tx| prepare_unread(tx, SESSION))
                .blocks
                .is_empty()
        );
    }

    #[test]
    fn requeue_refunds_once_and_old_generation_cannot_ack_the_new_owner() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let old = claim(&mut s, &prepared, &window, &[id]);
        assert!(crate::run(&mut s, NOW, |tx| release_reserved(tx, &old)).is_err());
        applied(ok(&mut s, |tx| {
            settle_reserved(tx, &old, Terminal::Requeue)
        }));
        assert_eq!(wakes::by_ids(s.conn(), &[id]).unwrap()[0].attempts, 0);
        let prepared = snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW).unwrap();
        let current = applied(ok(&mut s, |tx| {
            claim_reserved(
                tx,
                &prepared,
                std::slice::from_ref(&window),
                &[id],
                &[],
                Owner::new(2)?,
            )
        }));
        let before = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| ack_reserved_delivered(
                tx,
                &old,
                wakes::Riders::Release
            )),
            Decision::Stale
        );
        assert_eq!(
            ok(&mut s, |tx| ack_reserved_failed(tx, &old, "late", |_| None)),
            Decision::Stale
        );
        assert_eq!(
            ok(&mut s, |tx| settle_reserved(
                tx,
                &old,
                Terminal::Dropped("late")
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), before);
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &current, wakes::Riders::Release)
        }));
    }

    #[test]
    fn human_stop_keeps_zero_id_window_until_exact_late_delivery() {
        let (mut s, prepared, window) = fixture();
        let lease = claim(&mut s, &prepared, &window, &[]);
        ok(&mut s, |tx| wakes::human_stop(tx, SESSION));
        assert_eq!(
            load(s.conn(), SESSION)
                .unwrap()
                .unwrap()
                .active
                .unwrap()
                .phase,
            Phase::Cancelling
        );
        assert!(
            ok(&mut s, |tx| prepare_unread(tx, SESSION))
                .blocks
                .is_empty()
        );
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        }));
        assert_eq!(
            cursors::cursor(s.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.to_inclusive)
        );
    }

    #[test]
    fn cancelled_row_failure_cannot_retry_but_releases_exact_window() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let lease = claim(&mut s, &prepared, &window, &[id]);
        ok(&mut s, |tx| wakes::human_stop(tx, SESSION));
        let failed = applied(ok(&mut s, |tx| {
            ack_reserved_failed(tx, &lease, "late failure", |_| {
                panic!("cancelled row must not call retry policy")
            })
        }));
        assert!(failed[0].cancelled);
        assert_eq!(
            wakes::by_ids(s.conn(), &[id]).unwrap()[0].state,
            wakes::WakeState::Dropped
        );
        assert_eq!(
            cursors::cursor(s.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.from_exclusive)
        );
        assert!(
            !snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW)
                .unwrap()
                .blocks_channel(CHANNEL)
        );
    }

    #[test]
    fn boot_reset_preserves_attempts_and_reclaims_zero_id_owner() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let old = claim(&mut s, &prepared, &window, &[id]);
        let other_prepared = snapshot(s.conn(), "w-a2", &[CHANNEL.into()], NOW).unwrap();
        let zero = applied(ok(&mut s, |tx| {
            claim_reserved(
                tx,
                &other_prepared,
                std::slice::from_ref(&window),
                &[],
                &[],
                Owner::new(2)?,
            )
        }));
        let failed = ok(&mut s, |tx| {
            reset_inflight_and_reservations(tx, "synthetic restart", |n| {
                assert_eq!(n, 1);
                Some(NOW + 1)
            })
        });
        assert_eq!(failed.len(), 1);
        assert_eq!(wakes::by_ids(s.conn(), &[id]).unwrap()[0].attempts, 1);
        assert_eq!(ok(&mut s, |tx| release_reserved(tx, &old)), Decision::Stale);
        assert_eq!(
            ok(&mut s, |tx| release_reserved(tx, &zero)),
            Decision::Stale
        );
        assert_eq!(
            cursors::cursor(s.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.from_exclusive)
        );
        assert_eq!(
            cursors::cursor(s.conn(), "w-a2", CHANNEL).unwrap(),
            Some(window.from_exclusive)
        );
    }

    #[test]
    fn malformed_foreign_state_aborts_entire_boot_reclamation() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let _lease = claim(&mut s, &prepared, &window, &[id]);
        s.conn()
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,'malformed')",
                [format!("{PREFIX}w-a2")],
            )
            .unwrap();
        let before = durable_rows(&s);
        assert!(
            crate::run(&mut s, NOW, |tx| reset_inflight_and_reservations(
                tx,
                "restart",
                |_| None
            ))
            .is_err()
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn active_missing_cursor_is_invalid_and_pin_does_not_jump_the_frontier() {
        let (mut s, prepared, window) = fixture();
        let _lease = claim(&mut s, &prepared, &window, &[]);
        msg(
            &s,
            CHANNEL,
            "after-frontier",
            SenderKind::Session,
            Some("cap-a"),
            vec![],
        );
        assert_eq!(
            ok(&mut s, |tx| cursors::pin_to_end(tx, SESSION, CHANNEL)),
            window.from_exclusive
        );
        s.conn()
            .execute(
                "DELETE FROM read_cursors WHERE reader=?1 AND channel_id=?2",
                params![SESSION, CHANNEL],
            )
            .unwrap();
        let before = durable_rows(&s);
        assert!(crate::run(&mut s, NOW, |tx| prepare_unread(tx, SESSION)).is_err());
        assert!(crate::run(&mut s, NOW, |tx| cursors::pin_to_end(tx, SESSION, CHANNEL)).is_err());
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn other_channel_hook_commit_does_not_invalidate_current_exact_receipt() {
        let (mut s, prepared, window) = fixture();
        let lease = claim(&mut s, &prepared, &window, &[]);
        // A real second channel on this reader's injection surface, with an independent cursor.
        let (dm, _) = ok(&mut s, |tx| {
            crate::channels::ensure_dm(
                tx,
                "crew-a",
                &crewd_proto::MemberRef::Session { id: SESSION.into() },
                &crewd_proto::MemberRef::Session { id: "w-a2".into() },
            )
        });
        let end = msg(
            &s,
            &dm.id,
            "other-channel-body",
            SenderKind::Session,
            Some("w-a2"),
            vec![],
        );
        let hook = ok(&mut s, |tx| prepare_unread(tx, SESSION));
        assert!(!hook.blocks.iter().any(|(ch, _)| ch == CHANNEL));
        applied(ok(&mut s, |tx| {
            commit_hook(tx, &hook.snapshot, &[(dm.id.clone(), end)])
        }));
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        }));
        assert_eq!(
            cursors::cursor(s.conn(), SESSION, CHANNEL).unwrap(),
            Some(window.to_inclusive)
        );
    }

    #[test]
    fn strict_metadata_rejects_bad_identity_unknown_fields_and_overflow_numbers() {
        let (mut s, prepared, window) = fixture();
        let _lease = claim(&mut s, &prepared, &window, &[]);
        let key = format!("{PREFIX}{SESSION}");
        let original = crewd_store::meta_get(s.conn(), &key).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&original).unwrap();
        let mut cases = Vec::new();
        for (field, replacement) in [
            ("version", serde_json::json!(2)),
            ("sessionId", serde_json::json!("foreign")),
            ("incarnation", serde_json::json!("not-a-uuid")),
            ("revision", serde_json::json!(0)),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut v = value.clone();
            v[field] = replacement;
            cases.push(v.to_string());
        }
        for (field, replacement) in [
            ("token", serde_json::json!("")),
            ("executorGeneration", serde_json::json!(0)),
            ("phase", serde_json::json!("imagined")),
            ("wakeIds", serde_json::json!([1, 1])),
            ("windows", serde_json::json!([])),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut v = value.clone();
            v["active"][field] = replacement;
            cases.push(v.to_string());
        }
        let mut v = value.clone();
        v["active"]["windows"][0]["fromExclusive"] = serde_json::json!(-1);
        cases.push(v.to_string());
        cases.push(original.replace("\"revision\":1", "\"revision\":18446744073709551616"));
        for raw in cases {
            crewd_store::meta_set(s.conn(), &key, &raw).unwrap();
            assert!(
                matches!(
                    snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW),
                    Err(CollabError::Store(StoreError::Invalid(_)))
                ),
                "invalid envelope accepted: {raw}"
            );
        }
        crewd_store::meta_set(s.conn(), &key, &original).unwrap();
    }

    #[test]
    fn revision_exhaustion_aborts_claim_without_incrementing_attempts() {
        let (mut s, prepared, window) = fixture();
        let old = claim(&mut s, &prepared, &window, &[]);
        applied(ok(&mut s, |tx| release_reserved(tx, &old)));
        let key = format!("{PREFIX}{SESSION}");
        let mut state: serde_json::Value =
            serde_json::from_str(&crewd_store::meta_get(s.conn(), &key).unwrap().unwrap()).unwrap();
        state["revision"] = serde_json::json!(u64::MAX);
        crewd_store::meta_set(s.conn(), &key, &state.to_string()).unwrap();
        let prepared = snapshot(s.conn(), SESSION, &[CHANNEL.into()], NOW).unwrap();
        let id = wake(&mut s);
        let before = durable_rows(&s);
        let error = crate::run(&mut s, NOW, |tx| {
            claim_reserved(tx, &prepared, &[window], &[id], &[], Owner::new(2)?)
        })
        .unwrap_err();
        assert!(error.to_string().contains("revision overflow"));
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn own_leading_messages_and_oldest_cap_use_the_shared_scan() {
        let mut s = store();
        let own = msg(
            &s,
            CHANNEL,
            "leading-own",
            SenderKind::Session,
            Some(SESSION),
            vec![],
        );
        for i in 0..205 {
            msg(
                &s,
                CHANNEL,
                &format!("bounded-{i}"),
                SenderKind::Session,
                Some("cap-a"),
                vec![],
            );
        }
        let hook = ok(&mut s, |tx| prepare_unread(tx, SESSION));
        let messages = &hook.blocks.iter().find(|(ch, _)| ch == CHANNEL).unwrap().1;
        assert_eq!(messages.len(), cursors::UNREAD_CAP as usize);
        assert_eq!(messages.first().unwrap().id, "bounded-0");
        assert_eq!(messages.last().unwrap().id, "bounded-199");
        assert_eq!(hook.snapshot.cursor_base(CHANNEL), Some(Some(own)));
        let end = messages.last().unwrap().seq;
        applied(ok(&mut s, |tx| {
            commit_hook(tx, &hook.snapshot, &[(CHANNEL.into(), end)])
        }));
        let next = ok(&mut s, |tx| prepare_unread(tx, SESSION));
        assert_eq!(
            next.blocks
                .iter()
                .find(|(ch, _)| ch == CHANNEL)
                .unwrap()
                .1
                .len(),
            5
        );
    }

    #[test]
    fn active_hole_is_checked_before_a_leading_own_message_can_advance() {
        let (mut s, prepared, window) = fixture();
        let _lease = claim(&mut s, &prepared, &window, &[]);
        // Synthetic row mutation makes the first eligible row now own; never a caller proof.
        // It isolates guard ordering: the legacy scanner would immediately advance this row.
        s.conn()
            .execute(
                "UPDATE messages SET sender_session_id=?1 WHERE id='reserved-body'",
                [SESSION],
            )
            .unwrap();
        let before = durable_rows(&s);
        assert!(
            ok(&mut s, |tx| prepare_unread(tx, SESSION))
                .blocks
                .is_empty()
        );
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn foreign_reader_zero_id_window_blocks_archive_and_purge_but_state_survives() {
        let mut s = store();
        let reader = crate::testkit::org_worker(&mut s);
        let channel = "crew-x/lobby";
        ok(&mut s, |tx| {
            crate::listens::set(tx, &reader, channel, None, vec![])
        });
        let end = msg(
            &s,
            channel,
            "foreign-owned-window",
            SenderKind::Session,
            Some("cap-x"),
            vec![],
        );
        let prepared = snapshot(s.conn(), &reader, &[channel.into()], NOW).unwrap();
        let windows = prepared.windows_for(&[(channel.into(), end)]).unwrap();
        let lease = applied(ok(&mut s, |tx| {
            claim_reserved(tx, &prepared, &windows, &[], &[], Owner::new(1)?)
        }));
        let before = durable_rows(&s);
        let error = crate::run(&mut s, NOW, |tx| crate::crews::archive(tx, "crew-x")).unwrap_err();
        assert_eq!(
            error.refusal().unwrap().code,
            crate::refusal::Code::CrewWakesInflight
        );
        assert_eq!(durable_rows(&s), before);
        // Synthetic already-archived fixture proves the purge guard independently of archive.
        s.conn()
            .execute(
                "UPDATE crews SET archived_at=?1 WHERE id='crew-x'",
                [crewd_store::ts_from_ms(NOW)],
            )
            .unwrap();
        assert!(crate::run(&mut s, NOW, |tx| crate::crews::purge(tx, "crew-x")).is_err());
        applied(ok(&mut s, |tx| {
            settle_reserved(tx, &lease, Terminal::Dropped("confirmed not delivered"))
        }));
        let key = format!("{PREFIX}{reader}");
        let before_state = crewd_store::meta_get(s.conn(), &key).unwrap();
        crewd_store::meta_set(s.conn(), "unrelated.private.marker", "retain").unwrap();
        ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
        assert_eq!(crewd_store::meta_get(s.conn(), &key).unwrap(), before_state);
        assert_eq!(
            crewd_store::meta_get(s.conn(), "unrelated.private.marker")
                .unwrap()
                .as_deref(),
            Some("retain")
        );
        assert!(crate::sessions::get(s.conn(), &reader).is_ok());
    }

    #[test]
    fn orphan_state_delete_abort_rolls_back_actual_crew_cascades() {
        let mut s = store();
        let channel = "crew-x/lobby";
        let end = msg(
            &s,
            channel,
            "doomed-window",
            SenderKind::Session,
            Some("w-a1"),
            vec![],
        );
        let prepared = snapshot(s.conn(), "cap-x", &[channel.into()], NOW).unwrap();
        let windows = prepared.windows_for(&[(channel.into(), end)]).unwrap();
        let lease = applied(ok(&mut s, |tx| {
            claim_reserved(tx, &prepared, &windows, &[], &[], Owner::new(1)?)
        }));
        applied(ok(&mut s, |tx| release_reserved(tx, &lease)));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        let before = durable_rows(&s);
        s.conn().execute_batch("CREATE TEMP TRIGGER reservation_delete_abort BEFORE DELETE ON meta WHEN substr(OLD.key,1,22)='cursor.reservation.v1.' BEGIN SELECT RAISE(ABORT,'synthetic reservation delete abort'); END;").unwrap();
        let error = crate::run(&mut s, NOW, |tx| crate::crews::purge(tx, "crew-x")).unwrap_err();
        sql_abort(&error, "synthetic reservation delete abort");
        assert_eq!(durable_rows(&s), before);
        assert!(crate::crews::get(s.conn(), "crew-x").is_ok());
        assert!(crate::sessions::get(s.conn(), "cap-x").is_ok());
        assert!(
            crewd_store::messages::get(s.conn(), "doomed-window")
                .unwrap()
                .is_some()
        );
        s.conn()
            .execute_batch("DROP TRIGGER reservation_delete_abort")
            .unwrap();
        ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
        assert!(
            crewd_store::meta_get(s.conn(), &format!("{PREFIX}cap-x"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn row_prune_cannot_hide_an_active_receipt_with_a_premature_terminal_row() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let _lease = claim(&mut s, &prepared, &window, &[id]);
        s.conn()
            .execute(
                "UPDATE wakes SET state='delivered',settled_at=?1 WHERE id=?2",
                params![crewd_store::ts_from_ms(NOW - 100), id],
            )
            .unwrap();
        let before = durable_rows(&s);
        assert!(crate::run(&mut s, NOW, |tx| wakes::prune(tx, NOW, 100)).is_err());
        assert_eq!(durable_rows(&s), before);
    }

    #[test]
    fn failed_release_update_abort_rolls_back_retry_time_error_and_attempt() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let lease = claim(&mut s, &prepared, &window, &[id]);
        let before = durable_rows(&s);
        let rows = wakes::by_ids(s.conn(), &[id]).unwrap();
        s.conn().execute_batch("CREATE TEMP TRIGGER failed_release_abort BEFORE UPDATE ON meta WHEN substr(NEW.key,1,22)='cursor.reservation.v1.' BEGIN SELECT RAISE(ABORT,'synthetic failed release abort'); END;").unwrap();
        let error = crate::run(&mut s, NOW, |tx| {
            ack_reserved_failed(tx, &lease, "retry failure", |_| Some(NOW + 123))
        })
        .unwrap_err();
        sql_abort(&error, "synthetic failed release abort");
        assert_eq!(durable_rows(&s), before);
        assert_eq!(wakes::by_ids(s.conn(), &[id]).unwrap(), rows);
    }

    #[test]
    fn boot_release_update_abort_rolls_back_reset_and_zero_id_lease() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let _lease = claim(&mut s, &prepared, &window, &[id]);
        let other = snapshot(s.conn(), "w-a2", &[CHANNEL.into()], NOW).unwrap();
        let _zero = applied(ok(&mut s, |tx| {
            claim_reserved(tx, &other, &[window], &[], &[], Owner::new(2)?)
        }));
        let before = durable_rows(&s);
        let rows = wakes::by_ids(s.conn(), &[id]).unwrap();
        s.conn().execute_batch("CREATE TEMP TRIGGER boot_release_abort BEFORE UPDATE ON meta WHEN substr(NEW.key,1,22)='cursor.reservation.v1.' BEGIN SELECT RAISE(ABORT,'synthetic boot release abort'); END;").unwrap();
        let error = crate::run(&mut s, NOW, |tx| {
            reset_inflight_and_reservations(tx, "restart", |_| Some(NOW + 1))
        })
        .unwrap_err();
        sql_abort(&error, "synthetic boot release abort");
        assert_eq!(durable_rows(&s), before);
        assert_eq!(wakes::by_ids(s.conn(), &[id]).unwrap(), rows);
    }

    #[test]
    fn delivered_riders_keep_the_original_release_fate_and_refund() {
        let (mut s, prepared, window) = fixture();
        let id = wake(&mut s);
        let rider = match ok(&mut s, |tx| {
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: SESSION.into(),
                    kind: wakes::WakeKind::Timer,
                    source: wakes::source::timer("rider"),
                    priority: wakes::WakePriority::Auto,
                    not_before_ms: None,
                    note: None,
                    origin: None,
                },
            )
        }) {
            wakes::Enqueued::New(id) => id,
            other => panic!("{other:?}"),
        };
        let lease = applied(ok(&mut s, |tx| {
            claim_reserved(tx, &prepared, &[window], &[id], &[rider], Owner::new(1)?)
        }));
        applied(ok(&mut s, |tx| {
            ack_reserved_delivered(tx, &lease, wakes::Riders::Release)
        }));
        let rows = wakes::by_ids(s.conn(), &[id, rider]).unwrap();
        assert_eq!(
            rows.iter().find(|r| r.id == id).unwrap().state,
            wakes::WakeState::Delivered
        );
        let rider = rows.iter().find(|r| r.id == rider).unwrap();
        assert_eq!(rider.state, wakes::WakeState::Pending);
        assert_eq!(rider.attempts, 0);
    }

    /// Real post + successful real archive, without an active lease or wake-state rejection.
    /// crew-x has no children; crew-a would fail archive early on its live crew-b child.
    fn archived_own_lobby_preparation() -> (Store, Snapshot, Vec<Window>, i64) {
        let mut s = store();
        let reader = "cap-x";
        let channel = "crew-x/lobby";
        assert_eq!(
            crate::sessions::get(s.conn(), reader).unwrap().desired,
            crewd_proto::Desired::Stopped
        );
        let posted = ok(&mut s, |tx| {
            crate::messages::post(
                tx,
                crate::Sender::Human,
                crate::messages::NewPost {
                    channel_id: channel.into(),
                    text: "synthetic before-archive message".into(),
                    ..Default::default()
                },
            )
        });
        let seq = posted.message.seq;
        let prepared = snapshot(s.conn(), reader, &[channel.into()], NOW).unwrap();
        let windows = prepared.windows_for(&[(channel.into(), seq)]).unwrap();
        assert_eq!(windows.len(), 1);
        assert!(!prepared.blocks_channel(channel));
        assert!(load(s.conn(), reader).unwrap().is_none());
        assert_eq!(crate::testkit::count(&s, "SELECT COUNT(*) FROM wakes"), 0);
        let archived = ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        assert!(
            archived.removed_at.is_some(),
            "must exercise successful real archive"
        );
        assert!(
            crate::channels::get(s.conn(), channel)
                .unwrap()
                .archived_at
                .is_some()
        );
        assert_eq!(crate::testkit::count(&s, "SELECT COUNT(*) FROM wakes"), 0);
        assert!(load(s.conn(), reader).unwrap().is_none());
        assert_eq!(
            cursors::cursor(s.conn(), reader, channel).unwrap(),
            prepared.cursor_base(channel).unwrap()
        );
        (s, prepared, windows, seq)
    }

    #[test]
    fn prepared_zero_id_claim_is_stale_after_reader_crew_archive() {
        let (mut s, prepared, windows, _) = archived_own_lobby_preparation();
        // Archive's intentional writes are the baseline, not mutations attributed to claim.
        let after_archive = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &prepared,
                &windows,
                &[],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), after_archive);
    }

    #[test]
    fn prepared_hook_commit_is_stale_after_reader_crew_archive() {
        let (mut s, prepared, _, seq) = archived_own_lobby_preparation();
        let after_archive = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| commit_hook(
                tx,
                &prepared,
                &[("crew-x/lobby".into(), seq)]
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), after_archive);
    }

    #[test]
    fn fresh_archived_prepare_has_no_automatic_body_or_cursor_writes() {
        let (mut s, _, _, _) = archived_own_lobby_preparation();
        let after_archive = durable_rows(&s);
        let prepared = ok(&mut s, |tx| prepare_unread(tx, "cap-x"));
        assert!(prepared.blocks.is_empty());
        assert_eq!(durable_rows(&s), after_archive);
        assert_eq!(
            ok(&mut s, |tx| commit_hook(tx, &prepared.snapshot, &[])),
            Decision::Stale
        );
        // Retained-history membership is deliberately unchanged by the new automatic facade.
        assert!(
            crate::channels::channels_of_session(s.conn(), "cap-x")
                .unwrap()
                .iter()
                .any(|ch| ch.id == "crew-x/lobby")
        );
        assert_eq!(durable_rows(&s), after_archive);
    }

    #[test]
    fn real_unarchive_and_fresh_recapture_restore_ordinary_output() {
        let (mut s, _, _, seq) = archived_own_lobby_preparation();
        let restored = ok(&mut s, |tx| crate::crews::unarchive(tx, "crew-x"));
        assert!(restored.removed_at.is_none());
        assert!(
            crate::channels::get(s.conn(), "crew-x/lobby")
                .unwrap()
                .archived_at
                .is_none()
        );
        let prepared = ok(&mut s, |tx| prepare_unread(tx, "cap-x"));
        assert_eq!(
            prepared
                .blocks
                .iter()
                .find(|(ch, _)| ch == "crew-x/lobby")
                .unwrap()
                .1
                .iter()
                .map(|m| m.seq)
                .collect::<Vec<_>>(),
            vec![seq]
        );
        applied(ok(&mut s, |tx| {
            commit_hook(tx, &prepared.snapshot, &[("crew-x/lobby".into(), seq)])
        }));
        assert_eq!(
            cursors::cursor(s.conn(), "cap-x", "crew-x/lobby").unwrap(),
            Some(seq)
        );
    }

    #[test]
    fn empty_selected_timer_snapshot_is_stale_after_reader_crew_archive() {
        let mut s = store();
        let id = match ok(&mut s, |tx| {
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: "cap-x".into(),
                    kind: wakes::WakeKind::Timer,
                    source: wakes::source::timer("before-archive"),
                    priority: wakes::WakePriority::System,
                    not_before_ms: None,
                    note: Some("synthetic timer-only body".into()),
                    origin: None,
                },
            )
        }) {
            wakes::Enqueued::New(id) => id,
            other => panic!("{other:?}"),
        };
        let prepared = snapshot(s.conn(), "cap-x", &[], NOW).unwrap();
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        assert_eq!(
            wakes::by_ids(s.conn(), &[id]).unwrap()[0].state,
            wakes::WakeState::Dropped
        );
        let after_archive = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &prepared,
                &[],
                &[id],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        let fresh = snapshot(s.conn(), "cap-x", &[], NOW).unwrap();
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &fresh,
                &[],
                &[id],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        assert_eq!(durable_rows(&s), after_archive);
    }

    #[test]
    fn selected_actual_archived_channel_is_ineligible_even_with_live_reader_crew() {
        let (mut s, prepared, window) = fixture();
        // Instrument an independently archived own lobby, which public update disallows.
        // This is an exact branch guard test, not evidence of a public lifecycle sequence.
        s.conn()
            .execute(
                "UPDATE channels SET archived_at=?1 WHERE id=?2",
                params![crewd_store::ts_from_ms(NOW), CHANNEL],
            )
            .unwrap();
        assert!(!crate::crews::is_archived(s.conn(), "crew-a").unwrap());
        let after_archive = durable_rows(&s);
        assert_eq!(
            ok(&mut s, |tx| claim_reserved(
                tx,
                &prepared,
                &[window],
                &[],
                &[],
                Owner::new(1)?
            )),
            Decision::Stale
        );
        assert!(
            ok(&mut s, |tx| prepare_unread(tx, SESSION))
                .blocks
                .is_empty()
        );
        assert_eq!(durable_rows(&s), after_archive);
    }
}
