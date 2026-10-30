//! Seeded, synthetic Core state-machine walk. No actor loop, CLI or real service.
//! Effective binding: Task11b supplement plus the controller's post-634 resume
//! override. Only exact IDs of observed failed atomic claim/ack/settle may lose
//! ownership temporarily. Before/after rollback is checked first, any subsequent
//! successful operation retires the exemption, and actual boot rescue is checked
//! before clearing the remaining set. Human-stop cancellation is never exempt.
//! Message freshness is checked after a real qualifying delivery attempt (using
//! shared readiness/priority gates); an exact failed stale-row settlement must
//! retry on a subsequent fault-free attempt. No second readiness policy lives here.
//! Random crashes with outstanding RPC replies are explicitly skipped (coverage
//! reports selected/applied/skipped); pending-RPC abrupt death is not covered.
//! Eligible random crashes do not stop or settle first. Only end-run cleanup
//! uses Stop to release remaining replies, preserving the I8 contract.
//! Synthetic config uses loop thresholds 2/2, 250ms stagger, seeded retry jitter,
//! and advancing 1-2 message windows to exercise rare persistence interleavings.
//! Reproduce with CREWD_SIM_SEED=<decimal or 0xhex> CREWD_SIM_STEPS=<n> cargo test
//! -p crewd-server --lib orchestra::sim::sim_seeded_core_walk -- --exact --nocapture.
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt::Debug;
use std::time::Instant;

use crewd_exec::delivery::Subject;
use crewd_exec::health::{Health, HealthKind};
use crewd_exec::session::{
    BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, HeldDraft, Profile, SessionEvent,
};
use crewd_term::{DecisionEvent, DecisionKind, PendingDecision};
use crewd_wake::breaker::{HUMAN, SYSTEM, origin_of};
use crewd_wake::explain::{self, Blocker};
use crewd_wake::plan::{Listen, Mention, Sender};
use crewd_wake::queue::{self, MAX_ATTEMPTS};
use crewd_wake::text::Lang;
use crewd_wake::{
    HOUR, LatchScope, Ms, NewWake, Priority, QueuedWake, WakeKind, WakeState, source,
};
use tokio::sync::oneshot;

use super::clock::Clock;
use super::core::{Cmd, CollabFact, ControlError, Input, Origin, StartSession};
use super::notify::Key;
use super::port::{Ack, Audience, Batch, Port, Riders, SettingsUpdate, Settle};
use super::testkit::{MemFault, MemKind, Rig};

// The weights, operation names and enum live together: no parallel variant lists.
macro_rules! operations {
    ($($name:ident = $weight:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
        enum Op { $($name),+ }
        const OPERATIONS: &[(Op, u32)] = &[$((Op::$name, $weight)),+];
    };
}
operations! {
    HumanMention = 12, SessionMention = 10, Dm = 7, System = 7, Timer = 7,
    AskAnswer = 7, Listener = 7, Spawned = 2, WorkingOn = 5, WorkingOff = 10,
    DecisionAppear = 5, DecisionClear = 5, Delivered = 16, Blocked = 3,
    NotReady = 3, Stalled = 3, Undelivered = 6, Unsubmitted = 4, Abandoned = 3,
    HealthClear = 4, AuthRequired = 2, UsageLimit = 3, RateLimited = 3,
    LaunchFailed = 2, BriefUndelivered = 3, PromptUndelivered = 2,
    TurnFailed = 2, VersionIncompatible = 2, DraftText = 4, DraftUnknown = 4,
    DraftClear = 5, ResumeRefused = 2, Item = 1, TurnEnded = 1, Notice = 1,
    Profile = 1, UsageSnapshot = 1, ExitNatural = 4, ExitStopped = 3, ExitQuota = 3,
    ExitStoppedQuota = 2, StreamEnded = 3, LaunchedOk = 7, LaunchedErr = 4,
    Start = 3, Stop = 5, Resume = 12, NudgeText = 8, NudgeKeys = 5,
    Answer = 6, SetCaptain = 3, Recover = 3, SetSettings = 3, Hibernate = 2,
    Shutdown = 1, AdvanceSmall = 6, AdvanceBackoff = 7, AdvanceSixHours = 3,
    RestartAnswered = 5, FaultClaim = 3, FaultAck = 3, FaultAckCursor = 3,
    FaultRender = 3, FaultLatch = 3, FaultSettle = 3, Crash = 7,
    Progress = 3, BinaryReplaced = 1,
}

// The same splitmix64 recurrence as queue::Jitter. Test-local, no new dependency.
struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn index(&mut self, len: usize) -> usize {
        (self.next() % len as u64) as usize
    }
    fn operation(&mut self) -> Op {
        let total: u32 = OPERATIONS.iter().map(|(_, w)| w).sum();
        let mut draw = self.next() % u64::from(total);
        for &(op, weight) in OPERATIONS {
            if draw < u64::from(weight) {
                return op;
            }
            draw -= u64::from(weight);
        }
        unreachable!("positive weights cover the draw")
    }
}

// Exhaustive matches against the real event enums: a new event or outcome must
// be assigned a walk input. The coverage test consumes OPERATIONS, not a copy.
fn event_op(e: &SessionEvent) -> Op {
    match e {
        SessionEvent::Spawned { .. } => Op::Spawned,
        SessionEvent::Working(true) => Op::WorkingOn,
        SessionEvent::Working(false) => Op::WorkingOff,
        SessionEvent::Decision(DecisionEvent::Appeared(_)) => Op::DecisionAppear,
        SessionEvent::Decision(DecisionEvent::Cleared) => Op::DecisionClear,
        SessionEvent::Delivery { outcome, .. } => match outcome {
            DeliveryOutcome::Delivered => Op::Delivered,
            DeliveryOutcome::Blocked(_) => Op::Blocked,
            DeliveryOutcome::NotReady(_) => Op::NotReady,
            DeliveryOutcome::Stalled(_) => Op::Stalled,
            DeliveryOutcome::Undelivered(_) => Op::Undelivered,
            DeliveryOutcome::Unsubmitted(_) => Op::Unsubmitted,
            DeliveryOutcome::Abandoned(_) => Op::Abandoned,
        },
        SessionEvent::Health(None) => Op::HealthClear,
        SessionEvent::Health(Some(h)) => match h.kind {
            HealthKind::AuthRequired => Op::AuthRequired,
            HealthKind::UsageLimit => Op::UsageLimit,
            HealthKind::RateLimited => Op::RateLimited,
            HealthKind::LaunchFailed => Op::LaunchFailed,
            HealthKind::BriefUndelivered => Op::BriefUndelivered,
            HealthKind::PromptUndelivered => Op::PromptUndelivered,
            HealthKind::TurnFailed => Op::TurnFailed,
            HealthKind::VersionIncompatible => Op::VersionIncompatible,
        },
        SessionEvent::InputDraft(Some(HeldDraft::Text(_))) => Op::DraftText,
        SessionEvent::InputDraft(Some(HeldDraft::Unknown)) => Op::DraftUnknown,
        SessionEvent::InputDraft(None) => Op::DraftClear,
        SessionEvent::ResumeRefused { .. } => Op::ResumeRefused,
        SessionEvent::Item { .. } => Op::Item,
        SessionEvent::TurnEnded { .. }
        | SessionEvent::NativeTurnOutcome { .. }
        | SessionEvent::ManagedResponseFailed { .. } => Op::TurnEnded,
        SessionEvent::Notice(_) => Op::Notice,
        SessionEvent::CodexObservation { .. } => Op::Profile,
        SessionEvent::Profile(_) => Op::Profile,
        SessionEvent::Usage(_) => Op::UsageSnapshot,
        SessionEvent::Exited(info) => match (info.stopped_by_user, info.hit_limit) {
            (false, false) => Op::ExitNatural,
            (true, false) => Op::ExitStopped,
            (false, true) => Op::ExitQuota,
            (true, true) => Op::ExitStoppedQuota,
        },
    }
}

trait ReplyProbe {
    fn poll(&mut self) -> Result<Option<String>, String>;
}
impl<T: Debug> ReplyProbe for oneshot::Receiver<T> {
    fn poll(&mut self) -> Result<Option<String>, String> {
        match self.try_recv() {
            Ok(v) => Ok(Some(format!("{v:?}"))),
            Err(oneshot::error::TryRecvError::Empty) => Ok(None),
            Err(oneshot::error::TryRecvError::Closed) => {
                Err("I8_reply_channel_closed_without_answer".into())
            }
        }
    }
}
#[derive(Default)]
struct Replies {
    pending: Vec<(String, Box<dyn ReplyProbe>)>,
    resumes: Vec<(String, oneshot::Receiver<Result<(), ControlError>>)>,
    nudges: Vec<oneshot::Receiver<Result<(), ControlError>>>,
    nudge_ok: usize,
    nudge_error: usize,
    requested: usize,
    answered: usize,
}
impl Replies {
    fn add<T: Debug + 'static>(&mut self, label: String, rx: oneshot::Receiver<T>) {
        self.requested += 1;
        self.pending.push((label, Box::new(rx)));
    }
    fn add_resume(&mut self, session: String, rx: oneshot::Receiver<Result<(), ControlError>>) {
        self.requested += 1;
        self.resumes.push((session, rx));
    }
    fn add_nudge(&mut self, rx: oneshot::Receiver<Result<(), ControlError>>) {
        self.requested += 1;
        self.nudges.push(rx);
    }
    fn poll(&mut self) -> Result<Vec<String>, String> {
        let mut nudges = Vec::new();
        for mut rx in self.nudges.drain(..) {
            match rx.try_recv() {
                Ok(result) => {
                    self.answered += 1;
                    if result.is_ok() {
                        self.nudge_ok += 1;
                    } else {
                        self.nudge_error += 1;
                    }
                }
                Err(oneshot::error::TryRecvError::Empty) => nudges.push(rx),
                Err(oneshot::error::TryRecvError::Closed) => {
                    return Err("I8_reply_channel_closed_without_answer: Nudge".into());
                }
            }
        }
        self.nudges = nudges;
        let mut successful = Vec::new();
        let mut resumes = Vec::new();
        for (session, mut rx) in self.resumes.drain(..) {
            match rx.try_recv() {
                Ok(result) => {
                    self.answered += 1;
                    if result.is_ok() {
                        successful.push(session);
                    }
                }
                Err(oneshot::error::TryRecvError::Empty) => resumes.push((session, rx)),
                Err(oneshot::error::TryRecvError::Closed) => {
                    return Err(format!(
                        "I8_reply_channel_closed_without_answer: Resume {session}"
                    ));
                }
            }
        }
        self.resumes = resumes;
        let mut kept = Vec::new();
        for (name, mut rx) in self.pending.drain(..) {
            match rx.poll().map_err(|e| format!("{e}: {name}"))? {
                Some(_) => self.answered += 1,
                None => kept.push((name, rx)),
            }
        }
        self.pending = kept;
        Ok(successful)
    }
    fn complete(&mut self) -> Result<(), String> {
        self.poll()?;
        require(
            self.answered == self.requested
                && self.pending.is_empty()
                && self.resumes.is_empty()
                && self.nudges.is_empty(),
            "I8_all_replies_answered",
            format!(
                "{}/{}; pending {:?}",
                self.answered,
                self.requested,
                self.pending.iter().map(|(n, _)| n).collect::<Vec<_>>()
            ),
        )
    }
}

fn require(ok: bool, invariant: &str, detail: impl Debug) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{invariant}: {detail:?}"))
    }
}

#[derive(Default)]
struct Oracle {
    previous: BTreeMap<i64, QueuedWake>,
    fault_rows: BTreeSet<i64>,
    op_seen: usize,
    stops: BTreeMap<String, i64>,
    posts_seen: usize,
    notices: HashSet<Key>,
    notice_seen: usize,
    notice_post_cursor: usize,
    reclaimed: usize,
    owned_riders: BTreeSet<i64>,
    notice_actions: BTreeMap<String, usize>,
    exception_ids: BTreeSet<i64>,
    held_bypass_checks: usize,
    stale_attempts: usize,
    stale_checked: usize,
    stale_faulted: usize,
}
impl Oracle {
    async fn check(&mut self, r: &Rig) -> Result<(), String> {
        let rows = {
            let st = r.port.lock();
            let rows = st.wakes.clone();
            let mut owners: BTreeMap<i64, usize> = BTreeMap::new();
            for b in r
                .core
                .live
                .values()
                .filter_map(|l| l.inflight.as_ref().map(|i| &i.batch))
                .chain(r.core.starting.values().map(|s| &s.batch))
            {
                self.owned_riders.extend(b.riders.iter().map(|w| w.id));
                for id in b.ids() {
                    *owners.entry(id).or_default() += 1;
                }
            }
            for result in &st.op_results[self.op_seen..] {
                let Some(index) = result.fault_hit else {
                    for id in &result.ids {
                        self.fault_rows.remove(id);
                    }
                    continue;
                };
                let hit = &st.fault_hits[index];
                if matches!(
                    hit.fault,
                    MemFault::Claim | MemFault::Ack | MemFault::AckCursor | MemFault::Settle
                ) {
                    let invariant = if matches!(hit.fault, MemFault::Ack | MemFault::AckCursor) {
                        "I7_ack_rows_and_cursors_atomic"
                    } else {
                        "I1_fault_operation_is_atomic"
                    };
                    require(
                        hit.rows == hit.after_rows && hit.cursors == hit.after_cursors,
                        invariant,
                        hit,
                    )?;
                    self.fault_rows.extend(
                        hit.rows
                            .iter()
                            .filter(|w| w.state == WakeState::Inflight)
                            .map(|w| w.id),
                    );
                    self.exception_ids.extend(self.fault_rows.iter().copied());
                }
            }
            self.op_seen = st.op_results.len();
            for w in &rows {
                let count = owners.get(&w.id).copied().unwrap_or(0);
                // ONLY exact atomic claim/ack/settle hits may be temporarily orphaned; never
                // excuse multiple owners or a carried non-inflight row.
                require(
                    count <= 1 && (count == 0 || w.state == WakeState::Inflight),
                    "I1_carried_rows_are_inflight",
                    (w, count),
                )?;
                require(
                    w.state != WakeState::Inflight || count == 1 || self.fault_rows.contains(&w.id),
                    "I1_inflight_has_exactly_one_owner",
                    (w, count),
                )?;
                if let Some(old) = self.previous.get(&w.id)
                    && matches!(old.state, WakeState::Delivered | WakeState::Dropped)
                {
                    require(
                        w.state == old.state,
                        "I2_terminal_state_monotonic",
                        (old, w),
                    )?;
                }
                require(w.attempts <= MAX_ATTEMPTS, "I3_attempts_bounded", w)?;
                if let Some(floor) = self.stops.get(&w.session)
                    && r.core.live.contains_key(&w.session)
                    && !r.core.live[&w.session].stopping
                {
                    require(
                        rows.iter().any(|q| q.session == w.session && q.id > *floor),
                        "I5_human_stop_needs_new_wake",
                        (&w.session, floor),
                    )?;
                }
                if w.state == WakeState::Delivered
                    && w.kind == WakeKind::Message
                    && let Some(msg) = &w.msg
                {
                    let cursor = st
                        .cursors
                        .get(&(w.session.clone(), msg.channel.clone()))
                        .copied()
                        .unwrap_or(0);
                    require(
                        cursor >= msg.seq,
                        "I7_delivered_message_has_cursor",
                        (w, cursor),
                    )?;
                }
            }
            for (&id, &count) in &owners {
                require(
                    rows.iter().any(|w| w.id == id) && count == 1,
                    "I1_owner_points_to_existing_row",
                    (id, count),
                )?;
            }
            // Audience is exhaustively matched; also check the actual destination,
            // not only the enum submitted by Core.
            for (i, (to, _)) in st.posts.iter().enumerate().skip(self.posts_seen) {
                let Audience::HumanChannel { crew, .. } = to;
                let mut home = crew;
                while let Some(parent) = st.crew_parents.get(home) {
                    home = parent;
                }
                require(
                    st.posted[i].0 == format!("{home}/human"),
                    "I10_notices_only_human_channel",
                    &st.posted[i],
                )?;
            }
            for event in &r.core.notifier.transitions()[self.notice_seen..] {
                *self
                    .notice_actions
                    .entry(format!(
                        "{:?}:{}",
                        event.key.kind,
                        if event.raise { "raise" } else { "clear" }
                    ))
                    .or_default() += 1;
                if event.raise {
                    require(
                        self.notices.insert(event.key.clone()),
                        "I10_raise_once_between_clears",
                        event,
                    )?;
                } else {
                    require(
                        self.notices.remove(&event.key),
                        "I10_clear_has_a_real_raise",
                        event,
                    )?;
                }
                let expected = Audience::HumanChannel {
                    crew: event.crew.clone(),
                    mention_human: event.raise,
                };
                let offset = st.posts[self.notice_post_cursor..]
                    .iter()
                    .position(|(audience, text)| audience == &expected && text == &event.text);
                let offset = offset.ok_or_else(|| {
                    format!("I10_transition_has_successful_actual_post: {event:?}")
                })?;
                self.notice_post_cursor += offset + 1;
            }
            self.notice_seen = r.core.notifier.transitions().len();
            for key in &self.notices {
                require(
                    r.core.notifier.is_raised(key),
                    "I10_real_notifier_episode_matches",
                    key,
                )?;
            }
            self.posts_seen = st.posts.len();
            rows
        };
        {
            let exec = r.exec.lock();
            for (session, count) in &exec.process_counts {
                require(
                    (0..=1).contains(count),
                    "I4_one_process_per_session",
                    (session, count),
                )?;
                require(
                    (*count == 1) == exec.running.contains(session),
                    "I4_process_accounting_agrees",
                    (session, count),
                )?;
            }
        }
        let sessions = { r.port.lock().sessions.keys().cloned().collect::<Vec<_>>() };
        for session in sessions {
            let pending = r.port.pending(&session).await.map_err(|e| e.to_string())?;
            let top = r.core.top_due(&session, &pending, r.clock.now());
            let facts = r
                .core
                .target_facts(&session, &pending, top, &pending, r.clock.now())
                .await
                .map_err(|e| e.to_string())?;
            for w in pending
                .iter()
                .filter(|w| w.origin == HUMAN || w.kind == WakeKind::AskAnswer)
            {
                if !r.port.lock().latches.is_empty() {
                    self.held_bypass_checks += 1;
                }
                require(
                    r.core.breaker.held(&session, &w.origin).is_none(),
                    "I6_human_and_ask_answers_never_held",
                    w,
                )?;
                require(
                    !matches!(
                        explain::why_not_woken(
                            w,
                            top,
                            &facts,
                            r.core.breaker.held(&session, &w.origin),
                            r.clock.now()
                        ),
                        Some(Blocker::Held { .. })
                    ),
                    "I6_human_and_ask_answers_never_held",
                    w,
                )?;
            }
        }
        self.previous = rows.into_iter().map(|w| (w.id, w)).collect();
        Ok(())
    }
    async fn after_crash(&mut self, before: Vec<QueuedWake>, r: &Rig) -> Result<(), String> {
        // Actual fail_inflight result, before boot rescan can claim again.
        {
            let st = r.port.lock();
            let rescue = st
                .rescues
                .last()
                .ok_or("I1b_boot_did_not_call_port_reclamation")?;
            for old in before.iter().filter(|w| w.state == WakeState::Inflight) {
                require(
                    rescue.before.iter().any(|w| w == old),
                    "I1b_boot_observed_exact_orphan_row",
                    old,
                )?;
                let new = rescue
                    .after
                    .iter()
                    .find(|w| w.id == old.id)
                    .ok_or("I1b_reclaimed_row_missing")?;
                let canceled =
                    old.id <= st.human_stop_floors.get(&old.session).copied().unwrap_or(0);
                require(
                    new.state != WakeState::Inflight,
                    "I1b_port_reclaimed_actual_inflight",
                    (old, new),
                )?;
                require(
                    new.attempts == old.attempts,
                    "I3_boot_rescue_counts_attempt",
                    (old, new),
                )?;
                if canceled {
                    require(
                        new.state == WakeState::Dropped,
                        "I5_boot_drops_human_stop_old_rows",
                        (old, new),
                    )?;
                } else if old.attempts >= MAX_ATTEMPTS {
                    require(
                        new.state == WakeState::Dropped,
                        "I3_boot_rescue_gives_up_at_limit",
                        (old, new),
                    )?;
                } else {
                    require(
                        new.state == WakeState::Pending,
                        "I1b_uncanceled_boot_row_is_requeued",
                        (old, new),
                    )?;
                }
            }
            for id in &self.fault_rows {
                if let Some(old) = before
                    .iter()
                    .find(|w| w.id == *id && w.state == WakeState::Inflight)
                {
                    require(
                        rescue
                            .after
                            .iter()
                            .any(|w| w.id == old.id && w.state != WakeState::Inflight),
                        "I1b_exact_fault_exception_reclaimed",
                        old,
                    )?;
                    self.reclaimed += 1;
                }
            }
        }
        self.fault_rows.clear();
        self.notices.clear();
        self.notice_seen = 0;
        self.notice_post_cursor = self.posts_seen;
        self.check(r)
            .await
            .map_err(|e| format!("I1b_crash_boot_reclamation: {e}"))
    }
    async fn message_freshness(&mut self, r: &mut Rig) -> Result<Vec<String>, String> {
        let now = r.clock.now();
        let sessions = { r.port.lock().sessions.keys().cloned().collect::<Vec<_>>() };
        let mut attempts = Vec::new();
        for session in sessions {
            let pending = r.port.pending(&session).await.map_err(|e| e.to_string())?;
            let top = r.core.top_due(&session, &pending, now);
            let facts = r
                .core
                .target_facts(&session, &pending, top, &pending, now)
                .await
                .map_err(|e| e.to_string())?;
            let eligible: Vec<_> = {
                let st = r.port.lock();
                pending
                    .iter()
                    .filter(|w| queue::is_due(w, now) && w.kind == WakeKind::Message)
                    .filter(|w| {
                        w.msg.as_ref().is_some_and(|m| {
                            st.cursors
                                .get(&(session.clone(), m.channel.clone()))
                                .copied()
                                .unwrap_or(0)
                                >= m.seq
                        })
                    })
                    .filter(|w| {
                        explain::why_not_woken(
                            w,
                            top,
                            &facts,
                            r.core.breaker.held(&session, &w.origin),
                            now,
                        )
                        .is_none_or(|b| b.is_launch_gate())
                    })
                    .map(|w| w.id)
                    .collect()
            };
            if eligible.is_empty() {
                continue;
            }
            let before_faults = r.port.lock().fault_hits.len();
            // An actual production Core delivery attempt, not a render-count proxy.
            let result = r.core.try_deliver(&session).await;
            r.settle().await;
            self.stale_attempts += 1;
            attempts.push(format!(
                "I7 actual try_deliver {session} stale={eligible:?} result={result:?}"
            ));
            let st = r.port.lock();
            for id in &eligible {
                let failed = result.is_err()
                    && st.fault_hits[before_faults..].iter().any(|hit| {
                        hit.fault == MemFault::Settle && hit.rows.iter().any(|w| w.id == *id)
                    });
                if failed {
                    self.stale_faulted += 1;
                } else {
                    require_not_stale(&st.wakes, *id, now)?;
                    self.stale_checked += 1;
                }
            }
        }
        Ok(attempts)
    }
    async fn quiescence(&self, r: &Rig) -> Result<(), String> {
        let now = r.clock.now();
        let sessions = { r.port.lock().sessions.keys().cloned().collect::<Vec<_>>() };
        for session in sessions {
            let pending = r.port.pending(&session).await.map_err(|e| e.to_string())?;
            let top = r.core.top_due(&session, &pending, now);
            let facts = r
                .core
                .target_facts(&session, &pending, top, &pending, now)
                .await
                .map_err(|e| e.to_string())?;
            for w in pending.iter().filter(|w| queue::is_due(w, now)) {
                let reason = explain::why_not_woken(
                    w,
                    top,
                    &facts,
                    r.core.breaker.held(&session, &w.origin),
                    now,
                );
                require(reason.is_some(), "I9_due_row_has_live_oracle_blocker", w)?;
                let again = r
                    .core
                    .target_facts(&session, &pending, top, &pending, now)
                    .await
                    .map_err(|e| e.to_string())?;
                require(
                    reason
                        == explain::why_not_woken(
                            w,
                            top,
                            &again,
                            r.core.breaker.held(&session, &w.origin),
                            now,
                        ),
                    "I9_blocker_still_holds",
                    (w, reason),
                )?;
            }
        }
        Ok(())
    }
}

fn require_not_stale(rows: &[QueuedWake], id: i64, now: Ms) -> Result<(), String> {
    let row = rows
        .iter()
        .find(|w| w.id == id)
        .ok_or("I7_stale_row_disappeared_without_settlement")?;
    require(
        !queue::is_due(row, now),
        "I7_cursor_fresh_after_next_delivery_attempt",
        row,
    )
}

struct Walk {
    seed: u64,
    step: usize,
    phase: &'static str,
    trace: VecDeque<String>,
    oracle: Oracle,
    replies: Replies,
    selected: BTreeMap<Op, usize>,
    applied: BTreeMap<Op, usize>,
    matched_receipts: BTreeMap<Op, usize>,
    stale_receipts: BTreeMap<Op, usize>,
    fired_faults: BTreeMap<MemFault, usize>,
    latch_skip_reasons: BTreeMap<String, usize>,
    launch_ok: usize,
    launch_error: usize,
    rng: SplitMix64,
}
impl Walk {
    fn new(seed: u64) -> Self {
        Self {
            seed,
            step: 0,
            phase: "random",
            trace: VecDeque::new(),
            oracle: Oracle::default(),
            replies: Replies::default(),
            selected: BTreeMap::new(),
            applied: BTreeMap::new(),
            matched_receipts: BTreeMap::new(),
            stale_receipts: BTreeMap::new(),
            fired_faults: BTreeMap::new(),
            latch_skip_reasons: BTreeMap::new(),
            launch_ok: 0,
            launch_error: 0,
            rng: SplitMix64(seed),
        }
    }
    fn record(&mut self, input: impl Into<String>) {
        self.trace
            .push_back(format!("step={} {}", self.step, input.into()));
        while self.trace.len() > 60 {
            self.trace.pop_front();
        }
    }
    fn failed(&self, e: impl Debug) -> ! {
        panic!("{}", self.failure_report(e));
    }
    fn failure_report(&self, e: impl Debug) -> String {
        format!(
            "SIM seed={} step={} phase={} invariant={e:?}\nlast {} inputs:\n{}\nreproduce: CREWD_SIM_SEED={} CREWD_SIM_STEPS={}",
            self.seed,
            self.step,
            self.phase,
            self.trace.len(),
            self.trace.iter().cloned().collect::<Vec<_>>().join("\n"),
            self.seed,
            self.step + usize::from(self.phase == "random")
        )
    }
    fn poll_replies(&mut self) -> Result<(), String> {
        for session in self.replies.poll()? {
            self.oracle.stops.remove(&session);
        }
        Ok(())
    }
    async fn check(&mut self, r: &Rig) -> Result<(), String> {
        self.poll_replies()?;
        self.oracle.check(r).await
    }
    async fn freshness(&mut self, r: &mut Rig) -> Result<(), String> {
        for attempt in self.oracle.message_freshness(r).await? {
            self.record(attempt);
        }
        self.check(r).await
    }
    async fn command<T: Debug + 'static>(
        &mut self,
        r: &mut Rig,
        name: String,
        make: impl FnOnce(oneshot::Sender<T>) -> Cmd,
    ) {
        let (tx, rx) = oneshot::channel();
        self.replies.add(name.clone(), rx);
        self.record(name);
        r.core.handle(Input::Cmd(make(tx))).await;
        r.settle().await;
    }
    async fn stop(&mut self, r: &mut Rig, session: String) -> Result<(), String> {
        let floor = r.port.lock().next_wake;
        let (tx, mut rx) = oneshot::channel();
        self.record(format!("Cmd::Stop {session} wake_floor={floor}"));
        r.core
            .handle(Input::Cmd(Cmd::Stop {
                session: session.clone(),
                reply: tx,
            }))
            .await;
        match rx.try_recv() {
            Ok(result) => {
                self.replies.requested += 1;
                self.replies.answered += 1;
                if result.is_ok() {
                    self.oracle.stops.insert(session, floor);
                }
            }
            Err(_) => return Err("I8_stop_missing_immediate_reply".into()),
        }
        r.settle().await;
        Ok(())
    }

    // Read-only attribution: never consumes RNG, faults, rows or control inputs.
    fn latch_skip_state(&mut self, r: &Rig, reason: &str) {
        *self.latch_skip_reasons.entry(reason.into()).or_default() += 1;
        let now = r.clock.now();
        let st = r.port.lock();
        let faults = st.faults.clone();
        let mut target_rows = Vec::new();
        let mut sessions: Vec<_> = r.core.live.keys().cloned().collect();
        sessions.sort();
        for session in &sessions {
            let pending_due: Vec<_> = st
                .wakes
                .iter()
                .filter(|w| {
                    w.session == *session && w.state == WakeState::Pending && queue::is_due(w, now)
                })
                .map(|w| (w.id, w.kind, w.priority, w.not_before))
                .collect();
            let pending_future: Vec<_> = st
                .wakes
                .iter()
                .filter(|w| {
                    w.session == *session && w.state == WakeState::Pending && !queue::is_due(w, now)
                })
                .map(|w| (w.id, w.kind, w.priority, w.not_before))
                .collect();
            let inflight: Vec<_> = st
                .wakes
                .iter()
                .filter(|w| w.session == *session && w.state == WakeState::Inflight)
                .map(|w| w.id)
                .collect();
            target_rows.push(format!("target={session} reservation={} pending_due={pending_due:?} pending_future={pending_future:?} inflight_rows={inflight:?} render_error={:?} advance_error={:?} latch_error={:?} claim_race={:?}",
                st.reservations.contains_key(session), st.render_error.as_ref().or(st.render_error_for.get(session)), st.advance_error, st.latch_error, st.claim_race));
        }
        drop(st);
        let exec = r.exec.lock();
        let launch_faults = exec.fail_next.clone();
        for (session, rows) in sessions.iter().zip(target_rows) {
            let l = &r.core.live[session];
            eprintln!(
                "SIM FaultLatch skip seed={} step={} reason={reason} {rows} actual_live={{generation:{}, working:{}, health:{:?}, decision:{}, draft:{}, refused:{}, stopping:{}, halting:{}, writing:{}, queued_writes:{}, nudge_pending:{}, body_inflight:{}, restart_held:{}, system_held:{:?}}} outstanding={:?}",
                self.seed,
                self.step,
                l.generator,
                l.working,
                l.health,
                l.decision.is_some(),
                l.input_draft.is_some(),
                l.refused,
                l.stopping,
                l.halting,
                l.writing,
                l.writes.len(),
                l.nudge_pending.is_some(),
                l.inflight.is_some(),
                r.core.held.contains(session),
                r.core.breaker.held(session, SYSTEM),
                exec.outstanding.get(session)
            );
        }
        eprintln!(
            "SIM FaultLatch skip seed={} step={} reason={reason} faults={faults:?} launch_faults={launch_faults:?} shutdown={} hibernation={} live_count={} now={now:?}",
            self.seed,
            self.step,
            r.core.shutting_down,
            r.core.hibernation.is_some(),
            sessions.len()
        );
    }

    // A selected random FaultLatch is a bounded, same-Rig Core exercise, not
    // merely an armed hook. No separate fixture's counters enter this walk.
    async fn latch_fault_sequence(&mut self, r: &mut Rig) -> Result<bool, String> {
        if !r.port.lock().faults.is_empty()
            || !r.exec.lock().fail_next.is_empty()
            || r.core.shutting_down
            || r.core.hibernation.is_some()
        {
            self.record("FaultLatch skipped: another fault or shutdown is active");
            self.latch_skip_state(r, "operation:other_fault_or_shutdown");
            return Ok(false);
        }
        let mut candidates: Vec<_> = r.core.live.keys().cloned().collect();
        candidates.sort();
        let mut target = None;
        for session in candidates {
            let live = &r.core.live[&session];
            if live.health.is_some()
                || live.refused
                || live.writing
                || !live.writes.is_empty()
                || live.nudge_pending.is_some()
                || r.core.held.contains(&session)
                || r.core.breaker.held(&session, SYSTEM).is_some()
            {
                *self
                    .latch_skip_reasons
                    .entry("candidate:live_state_or_held".into())
                    .or_default() += 1;
                continue;
            }
            let clean = {
                let st = r.port.lock();
                !st.reservations.contains_key(&session)
                    && !st.wakes.iter().any(|w| {
                        w.session == session
                            && (w.state == WakeState::Inflight
                                || (w.state == WakeState::Pending
                                    && queue::is_due(w, r.clock.now())))
                    })
                    && st.render_error.is_none()
                    && !st.render_error_for.contains_key(&session)
                    && st.advance_error.is_none()
                    && st.latch_error.is_none()
                    && st.claim_race.is_none()
            };
            if !clean
                || r.exec
                    .lock()
                    .outstanding
                    .get(&session)
                    .is_some_and(|ids| !ids.is_empty())
            {
                *self
                    .latch_skip_reasons
                    .entry("candidate:rows_reservation_error_or_outstanding".into())
                    .or_default() += 1;
                continue;
            }
            let facts = r
                .core
                .target_facts(&session, &[], Some(Priority::System), &[], r.clock.now())
                .await
                .map_err(|e| e.to_string())?;
            if explain::live_blocker(&facts).is_none() {
                target = Some(session);
                break;
            }
            *self
                .latch_skip_reasons
                .entry(format!(
                    "candidate:shared_live_blocker:{:?}",
                    explain::live_blocker(&facts)
                ))
                .or_default() += 1;
            eprintln!(
                "SIM FaultLatch candidate seed={} step={} target={session} actual_target_facts={facts:?}",
                self.seed, self.step
            );
        }
        let Some(session) = target else {
            self.record("FaultLatch skipped: no originally ready, unheld, empty target");
            self.latch_skip_state(r, "operation:no_eligible_target");
            return Ok(false);
        };
        let generation = r.core.live[&session].generator;
        self.record(format!("FaultLatch begin target={session} generation={generation}; synthetic real-work Input::Progress"));
        r.core
            .handle(Input::Progress {
                session: session.clone(),
            })
            .await;
        r.settle().await;
        self.check(r).await?;
        let rounds = r.core.cfg.loops.edge_max.min(r.core.cfg.loops.target_max);
        require(rounds == 2, "FaultLatch_test_threshold_is_two", rounds)?;
        for round in 0..rounds {
            let id = self
                .latch_enqueue(r, &session, WakeKind::System, round)
                .await?;
            self.record(format!(
                "FaultLatch real try_deliver round={round} row={id}"
            ));
            let sent = r
                .core
                .try_deliver(&session)
                .await
                .map_err(|e| e.to_string())?;
            r.settle().await;
            require(sent, "FaultLatch_system_actually_sent", (round, id))?;
            self.check(r).await?;
            self.latch_ack(r, &session, generation, id).await?;
        }
        let blocked = self
            .latch_enqueue(r, &session, WakeKind::System, rounds)
            .await?;
        let before_hits = r.port.lock().fault_hits.len();
        let before_sends = r.exec.sent(&session).len();
        self.record(format!(
            "FaultLatch arm PutLatch then actual admit row={blocked}"
        ));
        r.port.fail_next(MemFault::PutLatch);
        self.check(r).await?;
        let result = r.core.try_deliver(&session).await;
        r.settle().await;
        require(result.is_err(), "FaultLatch_admit_must_fail", &result)?;
        self.check(r).await?;
        {
            let st = r.port.lock();
            require(
                st.fault_hits.len() == before_hits + 1
                    && st.fault_hits[before_hits].fault == MemFault::PutLatch,
                "FaultLatch_actual_single_hit",
                &st.fault_hits[before_hits..],
            )?;
            require(
                st.faults.is_empty(),
                "FaultLatch_consumed_not_cleared",
                &st.faults,
            )?;
            require(
                st.wakes
                    .iter()
                    .any(|w| w.id == blocked && w.state == WakeState::Pending),
                "FaultLatch_failed_row_pending",
                blocked,
            )?;
            require(
                st.latches
                    .iter()
                    .all(|l| l.id != crewd_wake::breaker::edge_id(SYSTEM, &session)),
                "FaultLatch_no_persisted_latch_on_error",
                &st.latches,
            )?;
        }
        require(
            r.core.breaker.held(&session, SYSTEM).is_none(),
            "FaultLatch_no_memory_latch_on_error",
            &session,
        )?;
        require(
            r.exec.sent(&session).len() == before_sends,
            "FaultLatch_no_send_on_error",
            &session,
        )?;
        self.record(format!(
            "FaultLatch real retry target={session} row={blocked}"
        ));
        let retry = r
            .core
            .try_deliver(&session)
            .await
            .map_err(|e| e.to_string())?;
        r.settle().await;
        require(!retry, "FaultLatch_retry_persists_without_send", &session)?;
        self.check(r).await?;
        let held = r
            .core
            .breaker
            .held(&session, SYSTEM)
            .cloned()
            .ok_or_else(|| "FaultLatch_retry_missing_memory_latch".to_string())?;
        require(
            r.port.lock().latches.contains(&held),
            "FaultLatch_same_target_durable_latch",
            &held,
        )?;
        require(
            r.exec.sent(&session).len() == before_sends,
            "FaultLatch_no_send_after_trip",
            &session,
        )?;

        self.record(format!(
            "FaultLatch Working(true) target={session}; hold Human/Ask for original I6"
        ));
        r.emit(&session, SessionEvent::Working(true)).await;
        self.check(r).await?;
        let ask = self
            .latch_enqueue(r, &session, WakeKind::AskAnswer, rounds + 1)
            .await?;
        self.record(format!("FaultLatch actual Human post target={session}"));
        let channel = format!("{}/lobby", r.core.live[&session].crew);
        let message = r
            .post(
                &channel,
                Sender::Human,
                vec![Mention::Session(session.clone())],
                "sim FaultLatch human display",
            )
            .await;
        self.check(r).await?;
        {
            let st = r.port.lock();
            require(
                st.latches.contains(&held),
                "FaultLatch_I6_same_target_persisted",
                &held,
            )?;
            require(
                st.wakes.iter().any(|w| {
                    w.session == session
                        && w.kind == WakeKind::AskAnswer
                        && w.id == ask
                        && w.state == WakeState::Pending
                }),
                "FaultLatch_I6_ask_pending",
                ask,
            )?;
            require(
                st.wakes.iter().any(|w| {
                    w.session == session
                        && w.origin == HUMAN
                        && w.msg.as_ref() == Some(&message)
                        && w.state == WakeState::Pending
                }),
                "FaultLatch_I6_human_pending",
                &message,
            )?;
            require(
                !st.reservations.contains_key(&session),
                "FaultLatch_display_before_any_lease",
                &session,
            )?;
        }
        self.record(format!("FaultLatch synthetic explicit displayed-prefix advance {session} {}#{}; not a helper stdout claim", message.channel, message.seq));
        r.port
            .advance(&session, &message.channel, message.seq)
            .await
            .map_err(|e| e.to_string())?;
        self.check(r).await?;
        let before_fresh = self.oracle.stale_checked;
        let settle_hits = r.port.lock().fault_hits.len();
        self.record(format!(
            "FaultLatch I7 arm one Settle then actual Working(false) {session}"
        ));
        r.port.fail_next(MemFault::Settle);
        r.emit(&session, SessionEvent::Working(false)).await;
        self.check(r).await?;
        {
            let st = r.port.lock();
            require(
                st.fault_hits.len() == settle_hits + 1
                    && st.fault_hits[settle_hits].fault == MemFault::Settle
                    && st.faults.is_empty(),
                "FaultLatch_I7_settle_consumed_not_cleared",
                &st.fault_hits[settle_hits..],
            )?;
        }
        self.record(format!("FaultLatch I7 actual freshness retry {session}"));
        self.freshness(r).await?;
        require(
            self.oracle.stale_checked > before_fresh,
            "FaultLatch_I7_actual_successful_check",
            self.oracle.stale_checked,
        )?;
        self.latch_ack(r, &session, generation, ask).await?;
        self.record(format!(
            "FaultLatch complete target={session}; actual hit, durable latch, I6 and I7"
        ));
        Ok(true)
    }

    async fn latch_enqueue(
        &mut self,
        r: &mut Rig,
        session: &str,
        kind: WakeKind,
        part: usize,
    ) -> Result<i64, String> {
        let token = format!("sim-latch-{}-{}-{part}", self.seed, self.step);
        let w = NewWake {
            session: session.into(),
            kind,
            source: if kind == WakeKind::System {
                source::system(&token)
            } else {
                format!("ask:{token}")
            },
            priority: if kind == WakeKind::AskAnswer {
                Priority::Human
            } else {
                Priority::System
            },
            not_before: r.clock.now(),
            note: Some(token),
            origin: if kind == WakeKind::AskAnswer {
                HUMAN.into()
            } else {
                SYSTEM.into()
            },
        };
        self.record(format!(
            "FaultLatch real enqueue {session} {kind:?} part={part}"
        ));
        let id = r
            .port
            .enqueue(w)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "FaultLatch_unique_enqueue_missing".to_string())?;
        r.settle().await;
        self.check(r).await?;
        Ok(id)
    }

    async fn latch_ack(
        &mut self,
        r: &mut Rig,
        session: &str,
        generation: u64,
        row: i64,
    ) -> Result<(), String> {
        let live = r
            .core
            .live
            .get(session)
            .ok_or_else(|| "FaultLatch_live_disappeared".to_string())?;
        let inf = live
            .inflight
            .as_ref()
            .ok_or_else(|| "FaultLatch_actual_inflight_missing".to_string())?;
        let delivery = inf
            .delivery
            .ok_or_else(|| "FaultLatch_actual_delivery_id_missing".to_string())?;
        require(
            live.generator == generation
                && inf.batch.wakes.len() == 1
                && inf.batch.wakes[0].id == row
                && inf.batch.riders.is_empty(),
            "FaultLatch_exact_current_row_no_old_body",
            (&inf.batch, generation),
        )?;
        require(
            matches!(
                inf.batch.wakes[0].kind,
                WakeKind::System | WakeKind::AskAnswer
            ) && inf.batch.wakes[0].origin
                == if inf.batch.wakes[0].kind == WakeKind::System {
                    SYSTEM
                } else {
                    HUMAN
                },
            "FaultLatch_exact_current_origin",
            &inf.batch.wakes,
        )?;
        require(
            inf.batch.cursor_targets.is_empty(),
            "FaultLatch_system_or_ask_no_cursor_body",
            &inf.batch,
        )?;
        let lease = inf
            .batch
            .lease
            .clone()
            .ok_or_else(|| "FaultLatch_exact_lease_missing".to_string())?;
        require(
            matches!(&lease, super::port::BatchLease::Memory { session: owner, generation: g, .. } if owner == session && *g == generation),
            "FaultLatch_exact_lease_generation",
            &lease,
        )?;
        require(
            r.port
                .lock()
                .reservations
                .get(session)
                .is_some_and(|b| b.lease.as_ref() == Some(&lease)),
            "FaultLatch_durable_exact_lease",
            &lease,
        )?;
        require(
            r.exec
                .lock()
                .outstanding
                .get(session)
                .is_some_and(|ids| ids.contains(&delivery)),
            "FaultLatch_actual_executor_receipt_id",
            delivery,
        )?;
        self.record(format!("FaultLatch real Delivered target={session} generation={generation} delivery={delivery} lease={lease:?} row={row}"));
        r.emit(
            session,
            SessionEvent::Delivery {
                id: delivery,
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        )
        .await;
        self.check(r).await?;
        require(
            r.port
                .lock()
                .wakes
                .iter()
                .any(|w| w.id == row && w.state == WakeState::Delivered),
            "FaultLatch_actual_ack_committed",
            row,
        )?;
        require(
            !r.port.lock().reservations.contains_key(session),
            "FaultLatch_ack_released_exact_lease",
            &lease,
        )?;
        Ok(())
    }
    // Select the nested fault sequence before entering the large ordinary-op
    // async dispatcher. In unoptimized builds their separate poll frames used
    // to stack (~0xbf000 + 0x79000) although the futures themselves were small.
    // Keep identical target/live RNG draws and all original fault assertions.
    fn apply<'a>(
        &'a mut self,
        r: &'a mut Rig,
        op: Op,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + 'a>> {
        let sessions: Vec<_> = r.port.lock().sessions.keys().cloned().collect();
        let session = sessions[self.rng.index(sessions.len())].clone();
        let lives: Vec<_> = sessions
            .iter()
            .filter(|s| r.core.live.contains_key(*s))
            .cloned()
            .collect();
        let live = if lives.is_empty() {
            None
        } else {
            Some(lives[self.rng.index(lives.len())].clone())
        };
        self.record(format!("{op:?} target={session} live={live:?}"));
        if op == Op::FaultLatch {
            Box::pin(self.latch_fault_sequence(r))
        } else {
            self.apply_selected(r, op, session, live)
        }
    }
    fn apply_selected<'a>(
        &'a mut self,
        r: &'a mut Rig,
        op: Op,
        session: String,
        live: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + 'a>> {
        match op {
            Op::HumanMention | Op::SessionMention | Op::Dm | Op::Listener => Box::pin(async move {
                let (channel, sender, mentions) = match op {
                    Op::HumanMention => (
                        "c/lobby",
                        Sender::Human,
                        vec![Mention::Session(session.clone())],
                    ),
                    Op::SessionMention => (
                        "c/lobby",
                        Rig::sender(if session == "w1" { "w2" } else { "w1" }),
                        vec![Mention::Session(session.clone())],
                    ),
                    Op::Dm => ("c/dm", Sender::Human, vec![]),
                    _ => {
                        r.port.lock().listens.insert(
                            "c/topic".into(),
                            vec![Listen {
                                session: session.clone(),
                                until: r.clock.now().add(6 * HOUR),
                                senders: None,
                            }],
                        );
                        ("c/topic", Sender::Human, vec![])
                    }
                };
                r.post(channel, sender, mentions, "sim message").await;
                if op == Op::HumanMention && self.rng.index(4) == 0 {
                    r.port.lock().window_cap = Some(1);
                }
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::System | Op::Timer | Op::AskAnswer => Box::pin(async move {
                let kind = match op {
                    Op::Timer => WakeKind::Timer,
                    Op::AskAnswer => WakeKind::AskAnswer,
                    _ => WakeKind::System,
                };
                let origin = origin_of(kind, None, &session);
                let token = format!("{}:{}", self.seed, self.step);
                let source = match kind {
                    WakeKind::Timer => source::timer(&token),
                    WakeKind::AskAnswer => format!("ask:{token}"),
                    _ => source::system(&token),
                };
                let priority = if op == Op::AskAnswer {
                    Priority::Human
                } else {
                    Priority::System
                };
                let delay = if self.rng.index(3) == 0 { 90_000 } else { 0 };
                let w = NewWake {
                    session: session.clone(),
                    kind,
                    source,
                    priority,
                    not_before: r.clock.now().add(delay),
                    note: Some("sim wake".into()),
                    origin,
                };
                r.port.enqueue(w).await.map_err(|e| e.to_string())?;
                r.core
                    .handle(Input::Collab(CollabFact::WakeEnqueued { session }))
                    .await;
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Start => Box::pin(async move {
                self.command(r, "Cmd::Start".into(), |reply| Cmd::Start {
                    req: StartSession {
                        codex_options: None,

                        crew_id: "c".into(),
                        captain: false,
                        executor: "claude".into(),
                        brief: "sim start".into(),
                        title: None,
                        model: None,
                        effort: None,
                        isolation: false,
                    },
                    origin: Origin::Human,
                    reply,
                })
                .await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Resume | Op::LaunchedOk | Op::LaunchedErr => Box::pin(async move {
                let launches_before = r.exec.lock().launch_outcomes.len();
                let targeted_session = session.clone();
                if op == Op::LaunchedErr {
                    r.exec.lock().fail_next.push("sim launch error".into());
                }
                // Explicit human Resume reopens a human stop; it is not an automatic relaunch.
                let (tx, rx) = oneshot::channel();
                self.replies.add_resume(session.clone(), rx);
                self.record(format!("Cmd::Resume {session} ({op:?})"));
                r.core
                    .handle(Input::Cmd(Cmd::Resume {
                        session,
                        origin: Origin::Human,
                        reply: tx,
                    }))
                    .await;
                r.settle().await;
                if matches!(op, Op::LaunchedOk | Op::LaunchedErr) {
                    let expected_ok = op == Op::LaunchedOk;
                    return Ok(r.exec.lock().launch_outcomes[launches_before..]
                        .iter()
                        .any(|(s, ok)| s == &targeted_session && *ok == expected_ok));
                }
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Stop => Box::pin(async move {
                if self.rng.index(2) == 0 {
                    r.exec.lock().hang_on_stop.insert(session.clone());
                }
                self.stop(r, session).await?;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::NudgeText | Op::NudgeKeys => Box::pin(async move {
                let input = if op == Op::NudgeText {
                    "sim nudge"
                } else {
                    ["", "enter", "2", "esc", "interrupt"][self.rng.index(5)]
                }
                .to_string();
                let (tx, rx) = oneshot::channel();
                self.replies.add_nudge(rx);
                self.record(format!("Cmd::Nudge {session} {input:?}"));
                r.core
                    .handle(Input::Cmd(Cmd::Nudge {
                        session,
                        input,
                        reply: tx,
                    }))
                    .await;
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Answer => Box::pin(async move {
                let fingerprint = r
                    .core
                    .live
                    .get(&session)
                    .and_then(|l| l.decision.as_ref())
                    .map(|d| d.fingerprint())
                    .unwrap_or_default();
                let fingerprint = if self.rng.index(4) == 0 {
                    "stale".into()
                } else {
                    fingerprint
                };
                let choice = self.rng.index(4);
                r.exec
                    .lock()
                    .box_script
                    .entry(session.clone())
                    .or_default()
                    .push_back(None);
                self.command(
                    r,
                    format!("Cmd::Answer {session} {fingerprint:?} choice={choice}"),
                    |reply| Cmd::Answer {
                        session,
                        fingerprint,
                        choice,
                        reply,
                    },
                )
                .await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::SetCaptain => Box::pin(async move {
                self.command(r, format!("Cmd::SetCaptain {session}"), |reply| {
                    Cmd::SetCaptain {
                        crew: "c".into(),
                        session,
                        reply,
                    }
                })
                .await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Recover => Box::pin(async move {
                let latches = r.port.lock().latches.clone();
                let (scope, id) = latches
                    .get(self.rng.index(latches.len().max(1)))
                    .map(|l| (l.scope, l.id.clone()))
                    .unwrap_or((LatchScope::Session, session));
                self.command(r, format!("Cmd::Recover {scope:?} {id}"), |reply| {
                    Cmd::Recover { scope, id, reply }
                })
                .await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::SetSettings => Box::pin(async move {
                let update = SettingsUpdate {
                    lang: Some(if self.rng.index(2) == 0 {
                        Lang::Zh
                    } else {
                        Lang::En
                    }),
                    max_live: Some([None, Some(0), Some(1), Some(4)][self.rng.index(4)]),
                    hibernate_deadline_secs: Some(Some(2)),
                };
                self.command(r, format!("Cmd::SetSettings {update:?}"), |reply| {
                    Cmd::SetSettings { update, reply }
                })
                .await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Hibernate => Box::pin(async move {
                let deadline_ms = Some(if self.rng.index(2) == 0 { 0 } else { 2_000 });
                self.command(r, format!("Cmd::Hibernate {deadline_ms:?}"), |reply| {
                    Cmd::Hibernate { deadline_ms, reply }
                })
                .await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Shutdown => Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                self.replies.add("Input::Shutdown".into(), rx);
                r.core.handle(Input::Shutdown(tx)).await;
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::AdvanceSmall | Op::AdvanceBackoff | Op::AdvanceSixHours => Box::pin(async move {
                let delta = match op {
                    Op::AdvanceSmall => 1 + self.rng.index(2_000) as i64,
                    Op::AdvanceBackoff => 180_000,
                    _ => 6 * HOUR + 1,
                };
                self.record(format!("clock+={delta}"));
                r.tick(delta).await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::RestartAnswered => Box::pin(async move {
                let resume = self.rng.index(2) == 0;
                if !self.restart_answer(r, resume).await {
                    return Ok(false);
                }
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::FaultLatch => Box::pin(async move {
                unreachable!("fault sequence dispatched before ordinary operation future")
            }),
            Op::FaultClaim
            | Op::FaultAck
            | Op::FaultAckCursor
            | Op::FaultRender
            | Op::FaultSettle => Box::pin(async move {
                let f = match op {
                    Op::FaultClaim => MemFault::Claim,
                    Op::FaultAck => MemFault::Ack,
                    Op::FaultAckCursor => MemFault::AckCursor,
                    Op::FaultRender => MemFault::RenderWake,
                    _ => MemFault::Settle,
                };
                r.port.fail_next(f);
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Progress => Box::pin(async move {
                r.core.handle(Input::Progress { session }).await;
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::BinaryReplaced => Box::pin(async move {
                r.core
                    .handle(Input::BinaryReplaced { crew: "c".into() })
                    .await;
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::StreamEnded => Box::pin(async move {
                let Some(live) = live else {
                    return Ok(false);
                };
                r.exec.end_stream(&live);
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
            Op::Crash => Box::pin(async move { unreachable!("crash consumes Rig in the driver") }),
            _ => Box::pin(async move {
                let Some(live) = live else {
                    return Ok(false);
                };
                let outstanding: Vec<_> = r
                    .exec
                    .lock()
                    .outstanding
                    .get(&live)
                    .map(|ids| ids.iter().copied().collect())
                    .unwrap_or_default();
                let delivery = outstanding
                    .get(self.rng.index(outstanding.len().max(1)))
                    .copied()
                    .unwrap_or(u64::MAX);
                let detail = "sim receipt".to_string();
                let event = match op {
                    Op::Spawned => SessionEvent::Spawned {
                        pid: 42,
                        agent_session_id: format!("sim-{}", self.step),
                        resumed: self.rng.index(2) == 0,
                    },
                    Op::WorkingOn => SessionEvent::Working(true),
                    Op::WorkingOff => SessionEvent::Working(false),
                    Op::DecisionAppear => {
                        SessionEvent::Decision(DecisionEvent::Appeared(PendingDecision {
                            kind: if self.rng.index(2) == 0 {
                                DecisionKind::Terminal
                            } else {
                                DecisionKind::RunnerRequest {
                                    request_id: format!("sim-{}", self.step),
                                }
                            },
                            prompt: format!("sim choice {}", self.step),
                            options: vec!["one".into(), "two".into()],
                            numbered: true,
                            selected: Some(0),
                        }))
                    }
                    Op::DecisionClear => SessionEvent::Decision(DecisionEvent::Cleared),
                    Op::Delivered
                    | Op::Blocked
                    | Op::NotReady
                    | Op::Stalled
                    | Op::Undelivered
                    | Op::Unsubmitted
                    | Op::Abandoned => SessionEvent::Delivery {
                        id: delivery,
                        subject: if delivery == BRIEF_DELIVERY_ID {
                            Subject::Brief
                        } else {
                            Subject::Steer
                        },
                        outcome: match op {
                            Op::Delivered => DeliveryOutcome::Delivered,
                            Op::Blocked => DeliveryOutcome::Blocked(detail),
                            Op::NotReady => DeliveryOutcome::NotReady(detail),
                            Op::Stalled => DeliveryOutcome::Stalled(detail),
                            Op::Undelivered => DeliveryOutcome::Undelivered(detail),
                            Op::Unsubmitted => DeliveryOutcome::Unsubmitted(detail),
                            _ => DeliveryOutcome::Abandoned(detail),
                        },
                    },
                    Op::HealthClear => SessionEvent::Health(None),
                    Op::AuthRequired
                    | Op::UsageLimit
                    | Op::RateLimited
                    | Op::LaunchFailed
                    | Op::BriefUndelivered
                    | Op::PromptUndelivered
                    | Op::TurnFailed
                    | Op::VersionIncompatible => SessionEvent::Health(Some(Health::new(
                        match op {
                            Op::AuthRequired => HealthKind::AuthRequired,
                            Op::UsageLimit => HealthKind::UsageLimit,
                            Op::RateLimited => HealthKind::RateLimited,
                            Op::LaunchFailed => HealthKind::LaunchFailed,
                            Op::BriefUndelivered => HealthKind::BriefUndelivered,
                            Op::PromptUndelivered => HealthKind::PromptUndelivered,
                            Op::TurnFailed => HealthKind::TurnFailed,
                            _ => HealthKind::VersionIncompatible,
                        },
                        "sim launch error",
                    ))),
                    Op::DraftText => SessionEvent::InputDraft(Some(HeldDraft::Text(format!(
                        "sim draft {}",
                        self.step
                    )))),
                    Op::DraftUnknown => SessionEvent::InputDraft(Some(HeldDraft::Unknown)),
                    Op::DraftClear => SessionEvent::InputDraft(None),
                    Op::ResumeRefused => SessionEvent::ResumeRefused {
                        id: "sim-old".into(),
                        said: "sim resume refused".into(),
                    },
                    Op::Item => SessionEvent::Item {
                        turn_id: "sim-turn".into(),
                        item: serde_json::json!({"type":"sim"}),
                    },
                    Op::TurnEnded => SessionEvent::TurnEnded {
                        turn_id: "sim-turn".into(),
                        last_agent_message: "sim end".into(),
                    },
                    Op::Notice => SessionEvent::Notice("sim notice".into()),
                    Op::Profile => SessionEvent::Profile(Profile::default()),
                    Op::UsageSnapshot => {
                        SessionEvent::Usage(crewd_exec::session::RunnerUsageSnapshot {
                            provider: "codex".into(),
                            thread_id: r
                                .live(&live)
                                .expect("selected live runner")
                                .agent_session_id
                                .clone(),
                            turn_id: None,
                            token_usage_json: "{}".into(),
                        })
                    }
                    Op::ExitNatural | Op::ExitStopped | Op::ExitQuota | Op::ExitStoppedQuota => {
                        SessionEvent::Exited(ExitInfo {
                            status: ExitStatusKind::Code(1),
                            stopped_by_user: matches!(op, Op::ExitStopped | Op::ExitStoppedQuota),
                            hit_limit: matches!(op, Op::ExitQuota | Op::ExitStoppedQuota),
                        })
                    }
                    _ => unreachable!("non-event operations handled above"),
                };
                require(
                    event_op(&event) == op,
                    "input_metadata_matches_event",
                    (&op, &event),
                )?;
                if let SessionEvent::Delivery { id, .. } = &event {
                    let counters = if outstanding.contains(id) {
                        &mut self.matched_receipts
                    } else {
                        &mut self.stale_receipts
                    };
                    *counters.entry(op).or_default() += 1;
                }
                self.record(format!("SessionEvent {live} {event:?}"));
                r.exec.emit(&live, event);
                r.settle().await;
                r.core.shutdown_done();
                Ok(true)
            }),
        }
    }
    async fn restart_answer(&mut self, r: &mut Rig, resume: bool) -> bool {
        if let Some(prompt) = r.core.restart_prompt {
            let sessions = r.core.held.iter().cloned().collect();
            r.port.lock().restart_answers.insert(prompt, resume);
            self.record(format!(
                "RestartAnswered {prompt} resume={resume} sessions={sessions:?}"
            ));
            r.core
                .handle(Input::Collab(CollabFact::RestartAnswered {
                    prompt,
                    resume,
                    sessions,
                }))
                .await;
            r.settle().await;
            return true;
        }
        false
    }
    async fn random_crash(&mut self, r: Rig) -> Result<(Rig, bool), String> {
        // An abrupt actor crash cannot answer an outstanding RPC. Keep I8 honest:
        // skip this selected input instead of changing durable intent with Stop.
        self.poll_replies()?;
        if !self.replies.pending.is_empty()
            || !self.replies.resumes.is_empty()
            || !self.replies.nudges.is_empty()
        {
            self.record("Crash skipped: outstanding reply; no stop, tick or settlement");
            return Ok((r, false));
        }
        self.record("Crash(drop Core, kill StandIn, same Port, boot)");
        self.check(&r).await?;
        let before = r.port.lock().wakes.clone();
        let r = r.crash().await.map_err(|e| e.to_string())?;
        self.oracle.after_crash(before, &r).await?;
        Ok((r, true))
    }

    // End-run cleanup releases pending RPCs with real control inputs. Random
    // crashes never call this: Stop would change durable cancellation intent.
    // No fabricated successful Delivery: stop/shutdown produce genuine Abandoned
    // and NotRunning replies through the existing StandIn/Core path.
    async fn release_replies(&mut self, r: &mut Rig) -> Result<(), String> {
        r.exec.lock().hang_on_stop.clear();
        let sessions: Vec<_> = r.core.live.keys().cloned().collect();
        for s in sessions {
            if r.core.live.get(&s).is_some_and(|l| {
                l.nudge_pending.is_some()
                    || l.writing
                    || !l.writes.is_empty()
                    || l.halting
                    || l.stopping
            }) {
                self.stop(r, s).await?;
            }
        }
        r.tick(3_000).await;
        r.core.shutdown_done();
        self.poll_replies()?;
        Ok(())
    }
}

// Heap-own the large synthetic walk before polling it. This keeps the
// unoptimized async state out of each test caller's stack; execution and
// all seed/step/cleanup assertions remain identical.
fn run(seed: u64, steps: usize) -> std::pin::Pin<Box<dyn std::future::Future<Output = Walk>>> {
    Box::pin(async move {
        let mut w = Walk::new(seed);
        // Small synthetic loop thresholds exercise the same real breaker/latch
        // persistence path within 400 steps; this is not a production settings edit.
        let mut cfg = super::OrchestraConfig::for_tests();
        cfg.jitter_seed = Some(seed);
        cfg.retry_jitter = queue::RETRY_JITTER;
        cfg.stagger_ms = 250;
        cfg.loops.edge_max = 2;
        cfg.loops.target_max = 2;
        let mut r = Rig::with_cfg(super::hooks::Hooks::default(), cfg).await;
        r.port.lock().window_cap = Some(2);
        r.crew("c", "cap");
        r.worker("c", "w1");
        r.worker("c", "w2");
        r.channel("c/dm", "c", None, &["w1", "w2"]);
        r.port.lock().channels.get_mut("c/dm").unwrap().kind = MemKind::Dm;
        r.channel("c/topic", "c", None, &["cap", "w1", "w2"]);
        for step in 0..steps {
            w.step = step;
            let op = w.rng.operation();
            *w.selected.entry(op).or_default() += 1;
            let result = if op == Op::Crash {
                let (next, applied) = match w.random_crash(r).await {
                    Ok(result) => result,
                    Err(e) => w.failed(e),
                };
                r = next;
                Ok(applied)
            } else {
                w.apply(&mut r, op).await
            };
            match result {
                Ok(true) => *w.applied.entry(op).or_default() += 1,
                Ok(false) => {}
                Err(e) => w.failed(e),
            }
            if let Err(e) = w.check(&r).await {
                w.failed(e);
            }
            if let Err(e) = w.freshness(&mut r).await {
                w.failed(e);
            }
        }
        w.step = steps;
        w.phase = "cleanup";
        r.port.faults_off();
        r.exec.lock().fail_next.clear();
        if let Err(e) = w.release_replies(&mut r).await {
            w.failed(e);
        }
        if let Err(e) = w.check(&r).await {
            w.failed(e);
        }
        // Final crash also proves every recorded orphan was reclaimed. It reopens a
        // shutdown/hibernate gate before the non-vacuous quiescence check.
        let before = r.port.lock().wakes.clone();
        w.record("cleanup crash+boot");
        r = match r.crash().await {
            Ok(r) => r,
            Err(e) => w.failed(e),
        };
        if let Err(e) = w.oracle.after_crash(before, &r).await {
            w.failed(e);
        }
        w.restart_answer(&mut r, true).await;
        r.tick(6 * HOUR + 180_001).await;
        if let Err(e) = w.check(&r).await {
            w.failed(e);
        }
        // Restart's silent rows and a newly started stagger can create another
        // runnable tier. Drive real ticks to quiescence; one large tick is not it.
        let bound = {
            let st = r.port.lock();
            st.wakes.len() + st.sessions.len() + 1
        };
        let mut quiescent = false;
        for round in 0..=bound {
            if w.oracle.quiescence(&r).await.is_ok() {
                quiescent = true;
                break;
            }
            w.record(format!(
                "quiescence tick round={round} past current stagger"
            ));
            r.tick(r.core.cfg.stagger_ms.max(0) + 1).await;
            if let Err(e) = w.check(&r).await {
                w.failed(e);
            }
        }
        if !quiescent {
            w.failed(w.oracle.quiescence(&r).await.unwrap_err());
        }
        w.record("cleanup Input::Shutdown");
        let (tx, rx) = oneshot::channel();
        w.replies.add("cleanup shutdown".into(), rx);
        r.core.handle(Input::Shutdown(tx)).await;
        r.settle().await;
        r.core.shutdown_done();
        if let Err(e) = w.check(&r).await {
            w.failed(e);
        }
        if let Err(e) = w.replies.complete() {
            w.failed(e);
        }
        for hit in &r.port.lock().fault_hits {
            *w.fired_faults.entry(hit.fault).or_default() += 1;
        }
        for (_, ok) in &r.exec.lock().launch_outcomes {
            if *ok {
                w.launch_ok += 1;
            } else {
                w.launch_error += 1;
            }
        }
        eprintln!(
            "SIM FaultLatch skip_reason_counts seed={seed} counts={:?}",
            w.latch_skip_reasons
        );
        w
    })
}

fn env_number(name: &str, default: usize) -> usize {
    std::env::var(name)
        .map(|v| {
            v.parse::<usize>()
                .expect("sim count must be a nonnegative integer")
        })
        .unwrap_or(default)
}
fn env_seed() -> Option<u64> {
    std::env::var("CREWD_SIM_SEED").ok().map(|v| {
        if let Some(hex) = v.strip_prefix("0x") {
            u64::from_str_radix(hex, 16).expect("hex seed")
        } else {
            v.parse().expect("decimal seed")
        }
    })
}
#[tokio::test]
async fn sim_seeded_core_walk() {
    let start = Instant::now();
    let steps = env_number("CREWD_SIM_STEPS", 400);
    let seeds: Vec<_> = env_seed()
        .map(|s| vec![s])
        .unwrap_or_else(|| (0..env_number("CREWD_SIM_SEEDS", 32) as u64).collect());
    assert!(
        !seeds.is_empty() && steps > 0,
        "zero seeds/steps is not a walk"
    );
    let mut selected = BTreeMap::new();
    let mut applied = BTreeMap::new();
    let mut matched = BTreeMap::new();
    let mut stale = BTreeMap::new();
    let mut faults = BTreeMap::new();
    let mut notices = BTreeMap::new();
    let mut exceptions = BTreeMap::new();
    let mut launches = (0, 0);
    let mut replies = (0, 0);
    let mut nudges = (0, 0);
    let mut freshness = (0, 0, 0);
    let mut reclaimed = 0;
    let mut riders = 0;
    let mut held_bypass_checks = 0;
    for &seed in &seeds {
        let w = run(seed, steps).await;
        for (op, n) in w.selected {
            *selected.entry(op).or_insert(0usize) += n;
        }
        for (op, n) in w.applied {
            *applied.entry(op).or_insert(0usize) += n;
        }
        for (op, n) in w.matched_receipts {
            *matched.entry(op).or_insert(0usize) += n;
        }
        for (op, n) in w.stale_receipts {
            *stale.entry(op).or_insert(0usize) += n;
        }
        for (kind, n) in w.fired_faults {
            *faults.entry(kind).or_insert(0usize) += n;
        }
        for (action, n) in w.oracle.notice_actions {
            *notices.entry(action).or_insert(0usize) += n;
        }
        exceptions.insert(seed, w.oracle.exception_ids);
        launches.0 += w.launch_ok;
        launches.1 += w.launch_error;
        replies.0 += w.replies.requested;
        replies.1 += w.replies.answered;
        nudges.0 += w.replies.nudge_ok;
        nudges.1 += w.replies.nudge_error;
        freshness.0 += w.oracle.stale_attempts;
        freshness.1 += w.oracle.stale_checked;
        freshness.2 += w.oracle.stale_faulted;
        held_bypass_checks += w.oracle.held_bypass_checks;
        reclaimed += w.oracle.reclaimed;
        riders += w.oracle.owned_riders.len();
    }
    let skipped: BTreeMap<_, _> = selected
        .iter()
        .map(|(&op, &n)| (op, n - applied.get(&op).copied().unwrap_or(0)))
        .collect();
    assert_eq!(selected.values().sum::<usize>(), seeds.len() * steps);
    eprintln!(
        "SIM seeds={seeds:?} steps_per_seed={steps} random_steps={} wall={:?}\nselected={selected:?}\napplied={applied:?}\nskipped={skipped:?}\nmatched_receipts={matched:?} stale_receipts={stale:?}\nfired_faults={faults:?} actual_launches_ok_err={launches:?} actual_notifier_actions={notices:?}\nreplies_requested_answered={replies:?} nudge_ok_error={nudges:?} freshness_attempts_checked_faulted={freshness:?} carried_riders={riders} reclaimed_fault_rows={reclaimed} human_ask_checks_with_persisted_latches={held_bypass_checks}\nexact_fault_exception_ids_by_seed={exceptions:?}",
        seeds.len() * steps,
        start.elapsed()
    );
    // A reproduction may intentionally be short. The complete default corpus
    // must actually apply every metadata input and fire every one-shot hook.
    if env_seed().is_none() && seeds.len() == 32 && steps == 400 {
        for &(op, _) in OPERATIONS {
            assert!(
                applied.get(&op).is_some_and(|n| *n > 0),
                "coverage input never applied: {op:?}"
            );
        }
        for fault in MemFault::ALL {
            assert!(
                faults.get(fault).is_some_and(|n| *n > 0),
                "coverage fault never fired: {fault:?}"
            );
        }
        assert!(nudges.0 > 0 && nudges.1 > 0 && riders > 0 && reclaimed > 0);
        assert!(freshness.0 > 0 && freshness.1 > 0 && held_bypass_checks > 0);
    }
}

#[tokio::test]
async fn sim_one_shot_claim_fault_is_atomic_and_records_exact_rows() {
    let r = Rig::new().await;
    r.crew("c", "cap");
    let id = r
        .port
        .enqueue(NewWake {
            session: "cap".into(),
            kind: WakeKind::System,
            source: "system:control".into(),
            priority: Priority::System,
            not_before: Ms(0),
            note: None,
            origin: SYSTEM.into(),
        })
        .await
        .unwrap()
        .unwrap();
    let wakes = r.port.wakes_of("cap");
    let batch = Batch {
        session: "cap".into(),
        wakes: wakes.clone(),
        riders: vec![],
        cursor_targets: vec![],
        ..Default::default()
    };
    r.port.fail_next(MemFault::Claim);
    assert!(
        r.port.claim(&batch).await.is_err(),
        "armed claim fault must fire"
    );
    assert_eq!(r.port.wakes_of("cap"), wakes, "failed claim is atomic");
    let hits = r.port.lock().fault_hits.clone();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rows.iter().map(|w| w.id).collect::<Vec<_>>(), [id]);
    r.port.claim(&batch).await.unwrap();
    assert_eq!(
        r.port.lock().fault_hits.len(),
        1,
        "one shot, not persistent"
    );
}

#[tokio::test]
async fn sim_all_fault_hooks_fire_once_and_preserve_atomic_rows() {
    let r = Rig::new().await;
    r.crew("c", "cap");
    r.port
        .enqueue(NewWake {
            session: "cap".into(),
            kind: WakeKind::System,
            source: "system:control".into(),
            priority: Priority::System,
            not_before: Ms(0),
            note: None,
            origin: SYSTEM.into(),
        })
        .await
        .unwrap();
    let batch = Batch {
        session: "cap".into(),
        wakes: r.port.wakes_of("cap"),
        riders: vec![],
        cursor_targets: vec![("c/lobby".into(), 1)],
        ..Default::default()
    };
    r.port.claim(&batch).await.unwrap();
    for fault in [MemFault::Ack, MemFault::AckCursor] {
        let before = r.port.wakes_of("cap");
        r.port.fail_next(fault);
        assert!(
            r.port
                .ack(
                    &batch,
                    Ack::Delivered {
                        riders: Riders::Release
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(r.port.wakes_of("cap"), before);
        assert!(r.port.lock().cursors.values().all(|c| *c == 0));
    }
    r.port
        .ack(
            &batch,
            Ack::Delivered {
                riders: Riders::Release,
            },
        )
        .await
        .unwrap();
    r.port.fail_next(MemFault::RenderWake);
    assert!(r.port.render_wake("cap", &batch.wakes).await.is_err());
    r.port.render_wake("cap", &batch.wakes).await.unwrap();
    let latch = crewd_wake::Latch {
        scope: LatchScope::Session,
        id: "cap".into(),
        reason: "sim".into(),
        at: Ms(0),
    };
    r.port.fail_next(MemFault::PutLatch);
    assert!(r.port.put_latch(&latch).await.is_err());
    assert!(r.port.lock().latches.is_empty());
    r.port.put_latch(&latch).await.unwrap();
    r.port.fail_next(MemFault::Settle);
    assert!(r.port.settle(&[], Settle::Requeue, Ms(0)).await.is_err());
    r.port.settle(&[], Settle::Requeue, Ms(0)).await.unwrap();
    assert_eq!(r.port.lock().fault_hits.len(), 5);
}

#[tokio::test]
async fn sim_oracle_rejects_unowned_inflight_and_terminal_revival() {
    let r = Rig::new().await;
    r.crew("c", "cap");
    r.port
        .enqueue(NewWake {
            session: "cap".into(),
            kind: WakeKind::System,
            source: "system:control".into(),
            priority: Priority::System,
            not_before: Ms(0),
            note: None,
            origin: SYSTEM.into(),
        })
        .await
        .unwrap();
    let mut oracle = Oracle::default();
    oracle.check(&r).await.unwrap();
    r.port.lock().wakes[0].state = WakeState::Inflight;
    assert!(
        oracle
            .check(&r)
            .await
            .unwrap_err()
            .contains("I1_inflight_has_exactly_one_owner")
    );
    r.port.lock().wakes[0].state = WakeState::Dropped;
    oracle.check(&r).await.unwrap();
    r.port.lock().wakes[0].state = WakeState::Pending;
    assert!(
        oracle
            .check(&r)
            .await
            .unwrap_err()
            .contains("I2_terminal_state_monotonic")
    );
}

#[test]
fn sim_weight_table_and_event_metadata_are_not_vacuous() {
    let unique: BTreeSet<_> = OPERATIONS
        .iter()
        .map(|(op, weight)| {
            assert!(*weight > 0);
            *op
        })
        .collect();
    assert_eq!(
        unique.len(),
        OPERATIONS.len(),
        "a repeated op silently biases the walk"
    );
    let mut rng = SplitMix64(0);
    let mut seen = BTreeSet::new();
    for _ in 0..100_000 {
        seen.insert(rng.operation());
    }
    assert_eq!(seen, unique, "every table entry must be selectable");
}

async fn minimal_stop_rig() -> Rig {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "w");
    r.port
        .enqueue(NewWake {
            session: "w".into(),
            kind: WakeKind::System,
            source: "system:minimal".into(),
            priority: Priority::System,
            not_before: r.clock.now(),
            note: Some("sim minimal".into()),
            origin: SYSTEM.into(),
        })
        .await
        .unwrap();
    r.core
        .handle(Input::Collab(CollabFact::WakeEnqueued {
            session: "w".into(),
        }))
        .await;
    r.settle().await;
    assert!(r.live("w").unwrap().inflight.is_some());
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Inflight);
    r
}

// Original seed3 regression name/history retained; the accepted 634 fix must
// cancel the old row durably. Exact failed-Settle ownership exception is narrow
// and tested separately from healthy orphan detection.
#[tokio::test]
async fn sim_minimal_settle_fault_orphans_batch_and_relaunches_human_stop() {
    let mut r = minimal_stop_rig().await;
    let floor = r.port.lock().next_wake;
    r.port.fail_next(MemFault::Settle);
    let result: Result<(), ControlError> = r
        .cmd(|reply| Cmd::Stop {
            session: "w".into(),
            reply,
        })
        .await;
    assert!(result.is_ok());
    let mut oracle = Oracle::default();
    oracle.stops.insert("w".into(), floor);
    oracle.check(&r).await.unwrap();
    assert_eq!(oracle.fault_rows, BTreeSet::from([floor]));
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Inflight);
    assert!(!r.core.live.contains_key("w"));
    let before = r.port.lock().wakes.clone();
    r.port.faults_off();
    r = r.crash().await.unwrap();
    oracle.after_crash(before, &r).await.unwrap();
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Dropped);
    assert_eq!(r.port.lock().next_wake, floor);
    assert!(!r.core.live.contains_key("w"));
    assert!(oracle.fault_rows.is_empty());
    assert_eq!(oracle.reclaimed, 1);
}

#[tokio::test]
async fn sim_minimal_stop_without_fault_keeps_terminal_row_and_does_not_relaunch() {
    let mut r = minimal_stop_rig().await;
    let result: Result<(), super::core::ControlError> = r
        .cmd(|reply| Cmd::Stop {
            session: "w".into(),
            reply,
        })
        .await;
    assert!(result.is_ok());
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Dropped);
    assert!(!r.core.live.contains_key("w"));
    r = r.crash().await.unwrap();
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Dropped);
    assert!(!r.core.live.contains_key("w"));
    Oracle::default().check(&r).await.unwrap();
}

#[tokio::test]
async fn sim_failed_settle_exception_is_exact_atomic_and_boot_reclaimed() {
    let mut r = minimal_stop_rig().await;
    r.port.fail_next(MemFault::Settle);
    r.cmd(|reply| Cmd::Stop {
        session: "w".into(),
        reply,
    })
    .await
    .unwrap();
    let mut oracle = Oracle::default();
    oracle.stops.insert("w".into(), 1);
    oracle.check(&r).await.unwrap();
    assert_eq!(oracle.fault_rows, BTreeSet::from([1]));
    let fresh = r
        .port
        .enqueue(NewWake {
            session: "w".into(),
            kind: WakeKind::System,
            source: "system:fresh-after-stop".into(),
            priority: Priority::System,
            not_before: r.clock.now(),
            note: Some("sim fresh".into()),
            origin: SYSTEM.into(),
        })
        .await
        .unwrap()
        .unwrap();
    // A different row has no failed-operation exemption.
    r.port
        .lock()
        .wakes
        .iter_mut()
        .find(|w| w.id == fresh)
        .unwrap()
        .state = WakeState::Inflight;
    assert!(
        oracle
            .check(&r)
            .await
            .unwrap_err()
            .contains("I1_inflight_has_exactly_one_owner")
    );
    r.port
        .lock()
        .wakes
        .iter_mut()
        .find(|w| w.id == fresh)
        .unwrap()
        .state = WakeState::Pending;
    let before = r.port.lock().wakes.clone();
    r.port.faults_off();
    r = r.crash().await.unwrap();
    oracle.after_crash(before, &r).await.unwrap();
    assert_eq!(
        r.port
            .wakes_of("w")
            .iter()
            .find(|w| w.id == 1)
            .unwrap()
            .state,
        WakeState::Dropped
    );
    assert!(
        r.live("w").is_some(),
        "a genuinely fresh wake may reopen the stop"
    );
    assert_eq!(oracle.reclaimed, 1);
}

#[tokio::test]
async fn sim_cursor_freshness_observes_real_attempt_and_rejects_stale_row() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "w");
    r.run("w").await;
    r.emit("w", SessionEvent::Working(true)).await;
    let message = r
        .post(
            "c/lobby",
            Sender::Human,
            vec![Mention::Session("w".into())],
            "sim stale",
        )
        .await;
    let id = r.port.wakes_of("w")[0].id;
    r.port
        .advance("w", &message.channel, message.seq)
        .await
        .unwrap();
    assert!(
        require_not_stale(&r.port.lock().wakes, id, r.clock.now())
            .unwrap_err()
            .contains("I7_cursor_fresh_after_next_delivery_attempt")
    );
    let mut oracle = Oracle::default();
    assert!(oracle.message_freshness(&mut r).await.unwrap().is_empty());
    r.port.fail_next(MemFault::Settle);
    r.emit("w", SessionEvent::Working(false)).await;
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Pending);
    oracle.check(&r).await.unwrap();
    r.port.faults_off();
    assert_eq!(oracle.message_freshness(&mut r).await.unwrap().len(), 1);
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Delivered);
    assert_eq!(oracle.stale_checked, 1);
    oracle.check(&r).await.unwrap();
}

#[tokio::test]
async fn sim_resume_stop_override_requires_a_successful_actual_reply() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "w");
    let mut w = Walk::new(0);
    w.stop(&mut r, "w".into()).await.unwrap();
    let (tx, rx) = oneshot::channel();
    r.core.handle(Input::Shutdown(tx)).await;
    r.settle().await;
    r.core.shutdown_done();
    rx.await.unwrap();
    let (tx, rx) = oneshot::channel();
    w.replies.add_resume("w".into(), rx);
    r.core
        .handle(Input::Cmd(Cmd::Resume {
            session: "w".into(),
            origin: Origin::Human,
            reply: tx,
        }))
        .await;
    r.settle().await;
    w.poll_replies().unwrap();
    assert!(
        w.oracle.stops.contains_key("w"),
        "a refusal is not an override"
    );
    r = r.crash().await.unwrap();
    let (tx, rx) = oneshot::channel();
    w.replies.add_resume("w".into(), rx);
    r.core
        .handle(Input::Cmd(Cmd::Resume {
            session: "w".into(),
            origin: Origin::Human,
            reply: tx,
        }))
        .await;
    r.settle().await;
    w.poll_replies().unwrap();
    assert!(!w.oracle.stops.contains_key("w"));
    assert!(r.live("w").is_some());
    w.replies.complete().unwrap();
}

#[tokio::test]
async fn sim_notifier_observes_typed_successful_raise_clear_episodes() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    let key = Key::new(super::notify::Kind::LaunchFailed, "cap");
    r.port
        .post_system(
            Audience::HumanChannel {
                crew: "c".into(),
                mention_human: false,
            },
            "unrelated sim text".into(),
        )
        .await
        .unwrap();
    r.core
        .notifier
        .raise(key.clone(), "c", "not a synthetic template".into())
        .await
        .unwrap();
    r.core
        .notifier
        .raise(key.clone(), "c", "different free text".into())
        .await
        .unwrap();
    let mut oracle = Oracle::default();
    oracle.check(&r).await.unwrap();
    assert_eq!(r.core.notifier.transitions().len(), 1);
    r.core
        .notifier
        .clear(&key, "a free-text recovery".into())
        .await
        .unwrap();
    r.core
        .notifier
        .raise(key.clone(), "c", "second actual episode".into())
        .await
        .unwrap();
    oracle.check(&r).await.unwrap();
    assert_eq!(r.core.notifier.transitions().len(), 3);
    assert_eq!(oracle.notice_actions.get("LaunchFailed:raise"), Some(&2));
    assert_eq!(oracle.notice_actions.get("LaunchFailed:clear"), Some(&1));
    oracle.notice_seen = 0;
    assert!(
        oracle
            .check(&r)
            .await
            .unwrap_err()
            .contains("I10_raise_once_between_clears")
    );
}

#[tokio::test]
async fn sim_oldest_window_riders_advance_after_real_cursor_ack() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "w");
    r.port.lock().window_cap = Some(1);
    r.run("w").await;
    r.emit("w", SessionEvent::Working(true)).await;
    for _ in 0..3 {
        r.post(
            "c/lobby",
            Sender::Human,
            vec![Mention::Session("w".into())],
            "sim window",
        )
        .await;
    }
    r.emit("w", SessionEvent::Working(false)).await;
    assert_eq!(
        r.live("w")
            .unwrap()
            .inflight
            .as_ref()
            .unwrap()
            .batch
            .riders
            .len(),
        2
    );
    for seq in 1..=3 {
        r.ack("w").await;
        assert_eq!(r.port.cursor("w", "c/lobby").await.unwrap(), seq);
        r.tick(0).await;
    }
    assert!(
        r.port
            .wakes_of("w")
            .iter()
            .all(|w| w.state == WakeState::Delivered)
    );
    Oracle::default().check(&r).await.unwrap();
}

#[tokio::test]
async fn sim_atomic_rollback_ruler_and_success_retire_old_fault_exemption() {
    let mut r = minimal_stop_rig().await;
    r.port.fail_next(MemFault::Settle);
    r.cmd(|reply| Cmd::Stop {
        session: "w".into(),
        reply,
    })
    .await
    .unwrap();
    let actual = r.port.lock().fault_hits[0].clone();
    r.port.lock().fault_hits[0].after_rows[0].state = WakeState::Pending;
    assert!(
        Oracle::default()
            .check(&r)
            .await
            .unwrap_err()
            .contains("I1_fault_operation_is_atomic")
    );
    r.port.lock().fault_hits[0] = actual;
    let mut oracle = Oracle::default();
    oracle.check(&r).await.unwrap();
    assert_eq!(oracle.fault_rows, BTreeSet::from([1]));
    r.port
        .settle(
            &[1],
            Settle::Dropped {
                error: "sim healed".into(),
            },
            r.clock.now(),
        )
        .await
        .unwrap();
    // Negative successful-settlement control: it cannot borrow an earlier error.
    r.port.lock().wakes[0].state = WakeState::Inflight;
    assert!(
        oracle
            .check(&r)
            .await
            .unwrap_err()
            .contains("I1_inflight_has_exactly_one_owner")
    );
    assert!(oracle.fault_rows.is_empty());
    r.port.lock().wakes[0].state = WakeState::Dropped;
    oracle.check(&r).await.unwrap();
}

#[tokio::test]
async fn sim_process_ruler_rejects_duplicate_not_hidden_by_running_set() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.run("cap").await;
    r.exec.lock().process_counts.insert("cap".into(), 2);
    assert!(
        Oracle::default()
            .check(&r)
            .await
            .unwrap_err()
            .contains("I4_one_process_per_session")
    );
    r.exec.lock().process_counts.insert("cap".into(), 1);
    Oracle::default().check(&r).await.unwrap();
}

#[tokio::test]
async fn sim_random_crash_with_pending_reply_does_not_insert_human_stop() {
    let mut r = minimal_stop_rig().await;
    let mut w = Walk::new(0);
    let (tx, rx) = oneshot::channel();
    w.replies.add_nudge(rx);
    r.core
        .handle(Input::Cmd(Cmd::Nudge {
            session: "w".into(),
            input: "sim pending nudge".into(),
            reply: tx,
        }))
        .await;
    r.settle().await;
    w.check(&r).await.unwrap();
    assert!(
        !w.replies.nudges.is_empty(),
        "control must retain a pending actual reply"
    );
    let before = r.port.lock().wakes.clone();
    let floors = r.port.lock().human_stop_floors.clone();
    let (mut r, applied) = w.random_crash(r).await.unwrap();
    assert!(
        !applied,
        "a pending RPC makes this crash ineligible; do not fabricate a stop"
    );
    assert_eq!(r.port.lock().human_stop_floors, floors);
    assert_eq!(r.port.lock().wakes, before);
    assert!(r.live("w").is_some());
    w.release_replies(&mut r).await.unwrap();
    w.replies.complete().unwrap();
}

#[tokio::test]
async fn sim_random_crash_keeps_stopping_inflight_until_actual_boot_rescue() {
    let mut r = minimal_stop_rig().await;
    r.exec.lock().hang_on_stop.insert("w".into());
    let mut w = Walk::new(0);
    w.stop(&mut r, "w".into()).await.unwrap();
    w.check(&r).await.unwrap();
    let before = r.port.lock().wakes.clone();
    let floors = r.port.lock().human_stop_floors.clone();
    assert_eq!(before[0].state, WakeState::Inflight);
    let (r, applied) = w.random_crash(r).await.unwrap();
    assert!(applied);
    assert_eq!(r.port.lock().human_stop_floors, floors);
    assert_eq!(
        r.port.lock().rescues.last().unwrap().before,
        before,
        "crash must not settle the inflight batch before boot observes it"
    );
    assert_eq!(r.port.wakes_of("w")[0].state, WakeState::Dropped);
}

#[tokio::test]
async fn sim_cleanup_replay_preserves_exact_random_prefix() {
    let w = run(0, 22).await;
    let report = w.failure_report("synthetic cleanup observation");
    assert!(
        report.contains("CREWD_SIM_SEED=0 CREWD_SIM_STEPS=22"),
        "{report}"
    );
    assert!(report.contains("phase=cleanup"), "{report}");
}

#[test]
fn sim_random_failure_replay_includes_the_failing_input() {
    let mut w = Walk::new(7);
    w.step = 21;
    let report = w.failure_report("synthetic random observation");
    assert!(
        report.contains("CREWD_SIM_SEED=7 CREWD_SIM_STEPS=22"),
        "{report}"
    );
    assert!(report.contains("phase=random"), "{report}");
}
