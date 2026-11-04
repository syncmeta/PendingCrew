//! One bounded administrative executor per immutable case; never Human authority.
use crate::{Result, Tx, admin_cases, judgments};
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug)]
pub struct AdminLaunchPermit {
    pub(crate) schedule_revision: Option<String>,
    case: String,
    crew: String,
    source: String,
    decision: String,
    fingerprint: String,
}
impl AdminLaunchPermit {
    pub fn case_id(&self) -> &str {
        &self.case
    }
    pub fn crew_id(&self) -> &str {
        &self.crew
    }
    pub fn source_session(&self) -> &str {
        &self.source
    }
    pub fn decision_id(&self) -> &str {
        &self.decision
    }
}
#[derive(Clone, Debug)]
pub struct ExecutionWitness {
    case: String,
    crew: String,
    session: String,
    source: String,
    attempt: String,
}
impl ExecutionWitness {
    pub fn case_id(&self) -> &str {
        &self.case
    }
    pub fn crew_id(&self) -> &str {
        &self.crew
    }
    pub fn session_id(&self) -> &str {
        &self.session
    }
    pub fn source_session_id(&self) -> &str {
        &self.source
    }
    pub fn attempt_id(&self) -> &str {
        &self.attempt
    }
}
pub struct Registration {
    pub session: Session,
    pub witness: ExecutionWitness,
}
/// Internal runtime evidence; callers cannot synthesize a different binding.
#[derive(Clone, Debug)]
pub struct ExecutionLease {
    witness: ExecutionWitness,
    generation: Option<u64>,
}
impl ExecutionLease {
    pub fn witness(&self) -> &ExecutionWitness {
        &self.witness
    }
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRevocationReason {
    AuthorityChanged,
    AuthorityUnknown,
    GenerationMismatch,
    TargetChanged,
    CaptainChanged,
    MissingWitness,
    ExecutionUnknown,
    SourceAdmissionLost,
    AdminAdmissionLost,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeReleaseReason {
    Resolved,
    HandoffAccepted,
}
#[derive(Clone, Debug)]
pub enum NativeLeaseState {
    Active(ExecutionLease),
    Released(ExecutionLease, NativeReleaseReason),
    Revoked(ExecutionLease, NativeRevocationReason),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRevocation {
    attempt: String,
    generation: Option<u64>,
    reason: NativeRevocationReason,
    at: i64,
    checksum: String,
}
fn captain_witness(c: &Connection, crew: &str) -> Result<String> {
    let crew = crewd_store::crews::get(c, crew)?.ok_or_else(invalid)?;
    let captain = crew
        .captain_session_id
        .as_deref()
        .map(|id| crewd_store::sessions::get(c, id))
        .transpose()?
        .flatten();
    if let Some(id) = crew.captain_session_id.as_deref() {
        let row = captain.as_ref().ok_or_else(invalid)?;
        if row.id != id
            || row.crew_id != crew.id
            || row.role != Role::Captain
            || row.resume_failed_at.is_some()
            || crate::wakes::automatic_work_paused(c, id)?
        {
            return Err(invalid());
        }
    }
    let stops = crew
        .captain_session_id
        .as_deref()
        .map(|id| -> Result<_> {
            Ok((
                judgments::stop_epoch(c, id)?,
                crate::wakes::human_stop_floor(c, id)?,
            ))
        })
        .transpose()?;
    judgments::fingerprint(&(crew.captain_session_id, stops))
}
fn revocation_checksum(r: &NativeRevocation) -> Result<String> {
    judgments::fingerprint(&(&r.attempt, r.generation, r.reason, r.at))
}
/// Only the trusted Core's native Profile event may call this with its live generation.
pub fn observe_native_profile(
    tx: &mut Tx<'_>,
    w: &ExecutionWitness,
    observed_generation: u64,
    model: Option<String>,
    effort: Option<String>,
) -> Result<bool> {
    let mut r = matching(tx.conn(), w)?;
    if r.view.phase != AdminCaseExecution::Running
        || generation(tx.conn(), w)? != Some(observed_generation)
        || matches!(
            native_lease(tx.conn(), w, tx.now_ms())?,
            NativeLeaseState::Revoked(_, _)
        )
    {
        return Ok(false);
    }
    let row = crewd_store::sessions::get(tx.conn(), w.session_id())?.ok_or_else(invalid)?;
    for (existing, observed) in [(&row.model, &model), (&row.effort, &effort)] {
        if let Some(observed) = observed
            && (observed.trim().is_empty()
                || existing
                    .as_ref()
                    .is_some_and(|existing| existing != observed))
        {
            return Ok(false);
        }
    }
    if model
        .as_ref()
        .is_some_and(|value| row.model.as_ref() != Some(value))
        || effort
            .as_ref()
            .is_some_and(|value| row.effort.as_ref() != Some(value))
    {
        crate::sessions::set_profile(tx, w.session_id(), model, effort)?;
    }
    r.context = environment(tx.conn(), &r)?;
    save(tx, &r)?;
    Ok(true)
}
/// DB authority only: the runtime additionally checks both admissions in this same transaction.
pub fn native_lease(c: &Connection, w: &ExecutionWitness, now: i64) -> Result<NativeLeaseState> {
    let r = matching(c, w)?;
    let lease = ExecutionLease {
        witness: w.clone(),
        generation: generation(c, w)?,
    };
    let revoked = |reason| Ok(NativeLeaseState::Revoked(lease.clone(), reason));
    if let Some(revocation) = &r.native_revocation {
        if revocation.attempt != w.attempt
            || revocation.generation != lease.generation
            || revocation.checksum != revocation_checksum(revocation)?
        {
            return Err(invalid());
        }
        return revoked(revocation.reason);
    }
    if !matches!(
        r.view.phase,
        AdminCaseExecution::Registered
            | AdminCaseExecution::Requested
            | AdminCaseExecution::Running
    ) {
        return revoked(NativeRevocationReason::ExecutionUnknown);
    }
    let Some(captain) = &r.native_captain else {
        return revoked(NativeRevocationReason::MissingWitness);
    };
    if environment(c, &r).ok().as_deref() != Some(r.context.as_str()) {
        return revoked(NativeRevocationReason::AuthorityChanged);
    }
    if captain_witness(c, &r.crew).ok().as_ref() != Some(captain) {
        return revoked(NativeRevocationReason::CaptainChanged);
    }
    // Released authority preserves the original object scope, not mutable work revisions.
    let source = admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: r.crew.clone(),
            id: r.case.clone(),
        },
    )?;
    if source.revision != r.source_revision {
        return revoked(NativeRevocationReason::TargetChanged);
    }
    for id in &r.targets {
        if scope_work(c, &r, id).is_err() {
            return revoked(NativeRevocationReason::TargetChanged);
        }
    }
    if crate::admin_case_outcome::status_at(c, &r.crew, &r.case, now)?
        .is_some_and(|v| v.phase == AdminCaseOutcomePhase::Resolved)
    {
        return Ok(NativeLeaseState::Released(
            lease,
            NativeReleaseReason::Resolved,
        ));
    }
    if crate::admin_case_handoff::current_accepted(c, &r.crew, &r.case, now)?.is_some() {
        return Ok(NativeLeaseState::Released(
            lease,
            NativeReleaseReason::HandoffAccepted,
        ));
    }
    if current(c, &r, now).is_err() {
        return revoked(NativeRevocationReason::TargetChanged);
    }
    Ok(NativeLeaseState::Active(lease))
}
/// Persist a terminal native revocation before stopping the matching physical generation.
/// Admission reasons must be checked by the runtime in this transaction; all DB reasons
/// are recomputed so an old observation cannot kill a newer valid checkpoint.
pub fn revoke_native(
    tx: &mut Tx<'_>,
    lease: &ExecutionLease,
    reason: NativeRevocationReason,
) -> Result<bool> {
    let mut r = matching(tx.conn(), &lease.witness)?;
    if generation(tx.conn(), &lease.witness)? != lease.generation {
        return Ok(false);
    }
    if r.native_revocation.is_some() {
        return Ok(false);
    }
    let reason = match reason {
        NativeRevocationReason::SourceAdmissionLost
        | NativeRevocationReason::AdminAdmissionLost
        | NativeRevocationReason::AuthorityUnknown
        | NativeRevocationReason::GenerationMismatch => reason,
        _ => match native_lease(tx.conn(), &lease.witness, tx.now_ms())? {
            NativeLeaseState::Revoked(_, current) => current,
            _ => return Ok(false),
        },
    };
    let mut receipt = NativeRevocation {
        attempt: r.view.attempt_id.clone(),
        generation: lease.generation,
        reason,
        at: tx.now_ms(),
        checksum: String::new(),
    };
    receipt.checksum = revocation_checksum(&receipt)?;
    r.native_revocation = Some(receipt);
    r.view.phase = AdminCaseExecution::Unknown;
    r.view.reason = AdminCaseExecutionReason::StateChanged;
    save(tx, &r)?;
    Ok(true)
}
#[derive(Clone, Debug)]
pub enum ReadScope {
    Work { id: String },
    SourceSession { id: String },
}
#[derive(Clone, Debug)]
pub enum ActionScope {
    WorkClose { id: String, kind: WorkClosureKind },
    WorkReassignLost { id: String, new_owner: String },
    WorkUnlink(Box<crate::admin_case_link::UnlinkParams>),
}
impl ActionScope {
    fn id(&self) -> &str {
        match self {
            Self::WorkClose { id, .. } | Self::WorkReassignLost { id, .. } => id,
            Self::WorkUnlink(p) => &p.id,
        }
    }
}
pub struct EffectPermit {
    witness: ExecutionWitness,
    action: ActionScope,
    core: crewd_work::WorkId,
    before_seq: u64,
    before_revision: String,
    checkpoint: String,
    context: String,
    link: Option<Box<crate::admin_case_link::ExpectedLink>>,
}

impl EffectPermit {
    pub(crate) fn apply_unlink(&self, tx: &mut Tx<'_>) -> Result<u64> {
        let link = self.link.as_ref().ok_or_else(invalid)?;
        let by = crate::Sender::Session(self.witness.session_id().into());
        let actor = crate::work::actor(tx.conn(), &by, self.witness.crew_id())?;
        crate::asks::with_book(tx, self.witness.crew_id(), |book, now| {
            book.relink(&actor, now, self.core, link.after.parent, link.after.depends_on.clone())
        }).map_err(crate::work::command_refusal)?;
        crate::work::changed(tx, &by, self.witness.crew_id(), self.action.id())?;
        self.before_seq.checked_add(1).ok_or_else(invalid)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    case: String,
    crew: String,
    source: String,
    decision: String,
    source_revision: String,
    targets: Vec<String>,
    checkpoint: String,
    context: String,
    view: AdminCaseExecutionView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_captain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_revocation: Option<NativeRevocation>,
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid admin case execution authority".into()).into()
}
fn key(id: &str) -> String {
    format!("admin.case.execution.v1:{id}")
}
fn session_key(id: &str) -> String {
    format!("admin.case.execution.session.v1:{id}")
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionMarker {
    version: u32,
    case_id: String,
    attempt_id: String,
}
fn session_marker(c: &Connection, id: &str) -> Result<Option<SessionMarker>> {
    let raw = c
        .query_row("SELECT extra FROM sessions WHERE id=?1", [id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    let Some(raw) = raw else { return Ok(None) };
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(StoreError::from)?;
    let Some(marker) = value.get("adminCaseExecutionV1") else {
        return Ok(None);
    };
    let marker: SessionMarker = serde_json::from_value(marker.clone()).map_err(StoreError::from)?;
    if marker.version != 1 {
        return Err(invalid());
    }
    Ok(Some(marker))
}
fn record(c: &Connection, id: &str) -> Result<Option<Saved>> {
    let Some(raw) = meta_get(c, &key(id))? else {
        return Ok(None);
    };
    let r: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if r.version != 1
        || r.case != id
        || r.crew.is_empty()
        || r.source.is_empty()
        || r.targets.is_empty()
        || r.view.phase == AdminCaseExecution::NotStarted
        || (r.view.phase == AdminCaseExecution::Running
            && r.view
                .generation
                .as_ref()
                .is_none_or(|g| g.parse::<u64>().ok().is_none_or(|n| n.to_string() != *g)))
        || uuid::Uuid::parse_str(&r.view.attempt_id).is_err()
        || meta_get(c, &session_key(&r.view.session_id))?.as_deref() != Some(id)
    {
        return Err(invalid());
    }
    let marker = session_marker(c, &r.view.session_id)?.ok_or_else(invalid)?;
    if marker.case_id != id || marker.attempt_id != r.view.attempt_id {
        return Err(invalid());
    }
    Ok(Some(r))
}
fn witness(r: &Saved) -> ExecutionWitness {
    ExecutionWitness {
        case: r.case.clone(),
        crew: r.crew.clone(),
        source: r.source.clone(),
        session: r.view.session_id.clone(),
        attempt: r.view.attempt_id.clone(),
    }
}
fn save(tx: &mut Tx<'_>, r: &Saved) -> Result<()> {
    meta_set(
        tx.conn(),
        &key(&r.case),
        &serde_json::to_string(r).map_err(StoreError::from)?,
    )?;
    Ok(())
}
pub fn binding(c: &Connection, crew: &str, id: &str) -> Result<Option<ExecutionWitness>> {
    let Some(r) = record(c, id)? else {
        return Ok(None);
    };
    if r.crew != crew {
        return Err(invalid());
    }
    Ok(Some(witness(&r)))
}
pub fn session_binding(c: &Connection, session: &str) -> Result<Option<ExecutionWitness>> {
    let index = meta_get(c, &session_key(session))?;
    let marker = session_marker(c, session)?;
    match (index, marker) {
        (None, None) => {
            // O(history) fallback, used only when both identity hints are absent.
            // Search valid JSON for this exact session, including duplicate-key
            // occurrences. Unrelated corrupt cases do not deny ordinary sessions.
            let prefix = "admin.case.execution.v1:";
            let found: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta m, json_tree(CASE WHEN json_valid(m.value) THEN m.value ELSE '{}' END) j WHERE m.key>=?1 AND m.key<?2 AND j.fullkey='$.view.sessionId' AND j.type='text' AND j.value=?3)",
                params![prefix, format!("{prefix}\u{10ffff}"), session], |row| row.get(0),
            )?;
            if found {
                return Err(invalid());
            }
            Ok(None)
        }
        (Some(id), Some(marker)) if id == marker.case_id => {
            let r = record(c, &id)?.ok_or_else(invalid)?;
            if r.view.session_id != session || r.view.attempt_id != marker.attempt_id {
                return Err(invalid());
            }
            Ok(Some(witness(&r)))
        }
        _ => Err(invalid()),
    }
}

pub fn launch_permit(tx: &mut Tx<'_>, crew: &str, id: &str) -> Result<Option<AdminLaunchPermit>> {
    if record(tx.conn(), id)?.is_some() {
        return Ok(None);
    }
    let Some((s, decision)) = admin_cases::execution_eligibility(tx.conn(), crew, id, tx.now_ms())?
    else {
        return Ok(None);
    };
    Ok(Some(AdminLaunchPermit {
        schedule_revision: None,
        case: id.into(),
        crew: crew.into(),
        source: s.case.source.session_id,
        decision,
        fingerprint: s.fingerprint,
    }))
}
fn environment(c: &Connection, r: &Saved) -> Result<String> {
    environment_for(c, r, true)
}
fn environment_for(c: &Connection, r: &Saved, live: bool) -> Result<String> {
    let a = crewd_store::sessions::get(c, &r.view.session_id)?.ok_or_else(invalid)?;
    let source = crewd_store::sessions::get(c, &r.source)?.ok_or_else(invalid)?;
    if (live && r.view.phase == AdminCaseExecution::Running && a.desired != Desired::Running)
        || a.role != Role::Admin
        || a.crew_id != r.crew
        || source.crew_id != r.crew
        || source.desired != Desired::Running
        || source.resume_failed_at.is_some()
        || (live && a.resume_failed_at.is_some())
        || crate::crews::is_archived(c, &r.crew)?
        || crate::wakes::automatic_work_paused(c, &a.id)?
        || crate::wakes::automatic_work_paused(c, &source.id)?
    {
        return Err(invalid());
    }
    judgments::fingerprint(&(
        crewd_store::judgment_state::epoch(c)?,
        judgments::configuration(c)?,
        &r.source,
        &r.crew,
        &a.id,
        &a.executor,
        &a.model,
        &a.effort,
        judgments::stop_epoch(c, &source.id)?,
        crate::wakes::human_stop_floor(c, &source.id)?,
        judgments::stop_epoch(c, &a.id)?,
        crate::wakes::human_stop_floor(c, &a.id)?,
    ))
}
pub fn register_and_bind(tx: &mut Tx<'_>, permit: &AdminLaunchPermit) -> Result<Registration> {
    if record(tx.conn(), &permit.case)?.is_some() {
        return Err(invalid());
    }
    let Some((s, decision)) =
        admin_cases::execution_eligibility(tx.conn(), &permit.crew, &permit.case, tx.now_ms())?
    else {
        return Err(invalid());
    };
    if decision != permit.decision
        || s.fingerprint != permit.fingerprint
        || s.case.source.session_id != permit.source
    {
        return Err(invalid());
    }
    crate::admin_case_launch::claim(tx, permit)?;
    let source = crewd_store::sessions::get(tx.conn(), &permit.source)?.ok_or_else(invalid)?;
    let crew = crewd_store::crews::get(tx.conn(), &permit.crew)?.ok_or_else(invalid)?;
    let session = crate::sessions::register(
        tx,
        crate::sessions::NewSession {
            crew_id: permit.crew.clone(),
            role: Role::Admin,
            executor: source.executor,
            model: source.model,
            effort: source.effort,
            working_directory: crew.working_directory,
            title: Some(crate::prompts::render_file(crate::Locale::Zh).get("notice.admin.title").into()),
            brief: Some(format!(
                "Administrative case {}. Inspect only its original in-scope work. Use factual evidence for closure or verified lost-owner reassignment. A tool result is not case resolution.",
                permit.case
            )),
            ..Default::default()
        },
    )?;
    let mut r = Saved {
        version: 1,
        case: permit.case.clone(),
        crew: permit.crew.clone(),
        source: permit.source.clone(),
        decision,
        source_revision: s.case.revision,
        targets: s
            .case
            .targets
            .into_iter()
            .filter(|t| t.scope == AdminCaseScope::InScope)
            .map(|t| t.id)
            .collect(),
        checkpoint: permit.fingerprint.clone(),
        context: String::new(),
        native_captain: Some(captain_witness(tx.conn(), &permit.crew)?),
        native_revocation: None,
        view: AdminCaseExecutionView {
            session_id: session.id.clone(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            phase: AdminCaseExecution::Registered,
            reason: AdminCaseExecutionReason::Registered,
            generation: None,
            effects_count: 0,
        },
    };
    r.targets.sort();
    r.targets.dedup();
    r.context = environment(tx.conn(), &r)?;
    if meta_get(tx.conn(), &session_key(&session.id))?.is_some() {
        return Err(invalid());
    }
    let marker = serde_json::to_string(&SessionMarker {
        version: 1,
        case_id: r.case.clone(),
        attempt_id: r.view.attempt_id.clone(),
    })
    .map_err(StoreError::from)?;
    tx.conn().execute("UPDATE sessions SET extra=json_set(COALESCE(extra,'{}'),'$.adminCaseExecutionV1',json(?1)) WHERE id=?2",params![marker,session.id])?;
    meta_set(tx.conn(), &session_key(&session.id), &r.case)?;
    save(tx, &r)?;
    crate::admin_case_launch::bound(tx, permit, &witness(&r))?;
    Ok(Registration {
        session,
        witness: witness(&r),
    })
}
fn matching(c: &Connection, w: &ExecutionWitness) -> Result<Saved> {
    let r = record(c, &w.case)?.ok_or_else(invalid)?;
    if r.crew != w.crew
        || r.source != w.source
        || r.view.session_id != w.session
        || r.view.attempt_id != w.attempt
    {
        return Err(invalid());
    }
    Ok(r)
}
fn current(c: &Connection, r: &Saved, now: i64) -> Result<()> {
    if r.native_revocation.is_some()
        || r.native_captain
            .as_ref()
            .is_some_and(|expected| captain_witness(c, &r.crew).ok().as_ref() != Some(expected))
    {
        return Err(invalid());
    }
    if environment(c, r)? != r.context {
        return Err(invalid());
    }
    let s = admin_cases::snapshot(c, &r.crew, &r.case, now)?;
    let mut original: Vec<_> = s
        .case
        .targets
        .iter()
        .filter(|t| t.scope == AdminCaseScope::InScope)
        .map(|t| t.id.clone())
        .collect();
    original.sort();
    original.dedup();
    if s.fingerprint != r.checkpoint
        || s.case.revision != r.source_revision
        || original != r.targets
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn claim_launch(tx: &mut Tx<'_>, w: &ExecutionWitness) -> Result<bool> {
    let mut r = matching(tx.conn(), w)?;
    if r.view.phase != AdminCaseExecution::Registered {
        return Ok(false);
    };
    current(tx.conn(), &r, tx.now_ms())?;
    r.view.phase = AdminCaseExecution::Requested;
    r.view.reason = AdminCaseExecutionReason::LaunchRequested;
    save(tx, &r)?;
    Ok(true)
}
pub fn confirm_started(tx: &mut Tx<'_>, w: &ExecutionWitness, generation: String) -> Result<bool> {
    let mut r = matching(tx.conn(), w)?;
    if r.view.phase != AdminCaseExecution::Requested {
        return Ok(false);
    };
    if generation.trim().is_empty() {
        return Err(invalid());
    }
    current(tx.conn(), &r, tx.now_ms())?;
    r.view.phase = AdminCaseExecution::Running;
    r.view.reason = AdminCaseExecutionReason::Started;
    r.view.generation = Some(generation);
    save(tx, &r)?;
    Ok(true)
}
pub fn mark_unknown(tx: &mut Tx<'_>, w: &ExecutionWitness) -> Result<bool> {
    let mut r = matching(tx.conn(), w)?;
    if !matches!(
        r.view.phase,
        AdminCaseExecution::Registered
            | AdminCaseExecution::Requested
            | AdminCaseExecution::Running
    ) {
        return Ok(false);
    }
    r.view.reason = if r.view.phase == AdminCaseExecution::Registered {
        AdminCaseExecutionReason::StateChanged
    } else {
        AdminCaseExecutionReason::LaunchUnknown
    };
    r.view.phase = AdminCaseExecution::Unknown;
    save(tx, &r)?;
    Ok(true)
}
pub fn recover_page(tx: &mut Tx<'_>, after: Option<String>) -> Result<(usize, Option<String>)> {
    let (items, next) = admin_cases::source_page(tx.conn(), after)?;
    let count = items.len();
    for i in items {
        let Ok(Some(mut r)) = record(tx.conn(), &i.id) else {
            continue;
        };
        if matches!(
            r.view.phase,
            AdminCaseExecution::Requested | AdminCaseExecution::Running
        ) {
            r.view.phase = AdminCaseExecution::Unknown;
            r.view.reason = AdminCaseExecutionReason::RestartUnknown;
            save(tx, &r)?;
        }
    }
    Ok((count, next))
}
fn running(c: &Connection, w: &ExecutionWitness, now: i64) -> Result<Saved> {
    let r = matching(c, w)?;
    if r.view.phase != AdminCaseExecution::Running || r.view.generation.is_none() {
        return Err(invalid());
    }
    current(c, &r, now)?;
    Ok(r)
}
fn scope_work(c: &Connection, r: &Saved, id: &str) -> Result<()> {
    if !r.targets.iter().any(|t| t == id) || crate::work::get(c, id)?.crew_id != r.crew {
        return Err(invalid());
    }
    Ok(())
}
pub fn validate_read(
    c: &Connection,
    w: &ExecutionWitness,
    scope: ReadScope,
    now: i64,
) -> Result<()> {
    let r = if crate::admin_case_handoff::writes_relinquished(c, w.case_id())?
        || crate::admin_case_outcome::status_at(c, w.crew_id(), w.case_id(), now)?
            .is_some_and(|v| v.phase == AdminCaseOutcomePhase::Resolved)
    {
        let r = matching(c, w)?;
        if r.native_revocation.is_some() || environment_for(c, &r, false)? != r.context {
            return Err(invalid());
        }
        r
    } else {
        running(c, w, now)?
    };
    match scope {
        ReadScope::Work { id } => scope_work(c, &r, &id),
        ReadScope::SourceSession { id } => {
            if id == r.source {
                Ok(())
            } else {
                Err(invalid())
            }
        }
    }
}
pub fn validate_effect(
    tx: &mut Tx<'_>,
    w: &ExecutionWitness,
    scope: ActionScope,
) -> Result<EffectPermit> {
    if crate::admin_case_handoff::writes_relinquished(tx.conn(), w.case_id())?
        || crate::admin_case_outcome::status_at(tx.conn(), w.crew_id(), w.case_id(), tx.now_ms())?
            .is_some_and(|v| v.phase == AdminCaseOutcomePhase::Resolved)
    {
        return Err(invalid());
    }
    let r = running(tx.conn(), w, tx.now_ms())?;
    scope_work(tx.conn(), &r, scope.id())?;
    if let ActionScope::WorkReassignLost { new_owner, .. } = &scope {
        let n = crewd_store::sessions::get(tx.conn(), new_owner)?.ok_or_else(invalid)?;
        if n.crew_id != r.crew
            || n.resume_failed_at.is_some()
            || crate::wakes::automatic_work_paused(tx.conn(), &n.id)?
        {
            return Err(invalid());
        }
    }
    let reference = crewd_store::object_refs::get(
        tx.conn(),
        crewd_store::object_refs::ObjectKind::Plan,
        crate::work::number(scope.id())?,
    )?
    .ok_or_else(invalid)?;
    let core = crate::work::core_plan(tx.conn(), &reference)?;
    let book = crate::asks::load_org(tx.conn())?;
    let link = if let ActionScope::WorkUnlink(p) = &scope {
        scope_work(tx.conn(), &r, &p.related_id)?;
        Some(Box::new(crate::admin_case_link::expected(tx.conn(), &book, &r.crew, core, p)?))
    } else {
        None
    };
    let before_seq = book.state().last_seq;
    let before_revision = crate::work::get(tx.conn(), scope.id())?.revision;
    Ok(EffectPermit {
        witness: w.clone(),
        action: scope,
        core,
        before_seq,
        before_revision,
        checkpoint: r.checkpoint,
        context: r.context,
        link,
    })
}
pub fn finish_effect(tx: &mut Tx<'_>, permit: EffectPermit) -> Result<()> {
    let mut r = matching(tx.conn(), &permit.witness)?;
    if r.view.phase != AdminCaseExecution::Running
        || r.checkpoint != permit.checkpoint
        || environment(tx.conn(), &r)? != permit.context
    {
        return Err(invalid());
    }
    let mut q = tx.conn().prepare(
        "SELECT seq,body FROM organization_work_events WHERE seq>?1 ORDER BY seq LIMIT 257",
    )?;
    let rows = q
        .query_map(
            [i64::try_from(permit.before_seq).map_err(|_| invalid())?],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    if rows.is_empty() || rows.len() > 256 {
        return Err(invalid());
    }
    let mut primary = None;
    let mut expected = permit.before_seq;
    for (seq, raw) in rows {
        let seq = u64::try_from(seq).map_err(|_| invalid())?;
        let e: crewd_work::Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
        expected = expected.checked_add(1).ok_or_else(invalid)?;
        if seq != expected
            || e.seq != seq
            || e.by.session().map(|s| s.as_str()) != Some(r.view.session_id.as_str())
        {
            return Err(invalid());
        }
        use crewd_work::EventKind as E;
        let valid = match (&permit.action, &e.kind) {
            (
                ActionScope::WorkClose {
                    kind: WorkClosureKind::Done,
                    ..
                },
                E::Done {
                    id,
                    evidence: Some(_),
                    note: Some(_),
                    ..
                },
            ) if *id == permit.core => primary.replace(seq).is_none(),
            (ActionScope::WorkClose { kind, .. }, E::Dropped { id, reason })
                if *id == permit.core && reason.evidence.is_some() =>
            {
                matches!(
                    (kind, reason.kind),
                    (WorkClosureKind::Duplicate, crewd_work::DropKind::Duplicate)
                        | (
                            WorkClosureKind::Superseded,
                            crewd_work::DropKind::Superseded
                        )
                ) && primary.replace(seq).is_none()
            }
            (
                ActionScope::WorkClose { .. },
                E::BlockerCleared {
                    blocker: crewd_work::Blocker::Item(id),
                    ..
                },
            ) => *id == permit.core,
            (
                ActionScope::WorkReassignLost { new_owner, .. },
                E::OwnerChanged {
                    id,
                    to,
                    how: crewd_work::HowChanged::Lost,
                    ..
                },
            ) if *id == permit.core && to.as_str() == new_owner => primary.replace(seq).is_none(),
            (ActionScope::WorkUnlink(_), E::Relinked { id, parent, depends_on })
                if *id == permit.core && permit.link.as_ref().is_some_and(|link|
                    *parent == link.after.parent && *depends_on == link.after.depends_on) =>
                primary.replace(seq).is_none(),
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    let book = crate::asks::load_org(tx.conn())?;
    if primary != permit.before_seq.checked_add(1) || book.state().last_seq != expected {
        return Err(invalid());
    }
    let actual = crate::work::get(tx.conn(), permit.action.id())?;
    if actual.revision == permit.before_revision {
        return Err(invalid());
    }
    let result_matches = match &permit.action {
        ActionScope::WorkClose { kind, .. } => {
            actual.closure.as_ref().is_some_and(|c| &c.kind == kind)
                && actual.status
                    == if *kind == WorkClosureKind::Done {
                        "done"
                    } else {
                        "dropped"
                    }
        }
        ActionScope::WorkReassignLost { new_owner, .. } => {
            actual.owner.as_deref() == Some(new_owner.as_str())
        }
        ActionScope::WorkUnlink(_) => permit.link.as_ref().is_some_and(|link| link.matches(&book)),
    };
    if !result_matches {
        return Err(invalid());
    }
    let after = admin_cases::snapshot(tx.conn(), &r.crew, &r.case, tx.now_ms())?;
    r.checkpoint = after.fingerprint;
    r.view.effects_count = r.view.effects_count.checked_add(1).ok_or_else(invalid)?;
    let wake_id = match &permit.action {
        ActionScope::WorkReassignLost { new_owner, .. } => {
            crate::work_lost::assignment_notification(
                tx.conn(),
                permit.action.id(),
                new_owner,
                &actual.revision,
            )?
            .map(|row| row.id)
        }
        _ => None,
    };
    let receipt = EffectReceipt {
        case_id: r.case.clone(),
        session_id: r.view.session_id.clone(),
        work_id: permit.action.id().into(),
        first_event: permit.before_seq + 1,
        last_event: expected,
        checkpoint: r.checkpoint.clone(),
        result_revision: Some(actual.revision),
        wake_id,
        link_request_id: match &permit.action {
            ActionScope::WorkUnlink(p) => Some(crate::resource_acl::uuid(&p.request_id)?),
            _ => None,
        },
    };
    meta_set(
        tx.conn(),
        &effect_key(&r.view.attempt_id, r.view.effects_count),
        &serde_json::to_string(&receipt).map_err(StoreError::from)?,
    )?;
    save(tx, &r)
}
pub(crate) fn view(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<AdminCaseExecutionView>> {
    let Some(mut r) = record(c, id)? else {
        return Ok(None);
    };
    if r.crew != crew {
        return Err(invalid());
    }
    if !matches!(
        r.view.phase,
        AdminCaseExecution::Unknown | AdminCaseExecution::Stale
    ) && current(c, &r, now).is_err()
    {
        r.view.phase = AdminCaseExecution::Stale;
        r.view.reason = AdminCaseExecutionReason::StateChanged;
    }
    Ok(Some(r.view))
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        Sender, admin_cases, stop_checks,
        testkit::{NOW, ok, store},
        work,
    };
    pub(crate) fn adopted(s: &mut crewd_store::Store) -> AdminCaseView {
        adopted_with_edge(s, None)
    }
    fn adopted_with_edge(s: &mut crewd_store::Store, edge: Option<crate::admin_case_link::Edge>) -> AdminCaseView {
        let settings = crewd_proto::judgments::JudgmentSettings {
            revision: Some("synthetic".into()),
            config: Some(crewd_proto::judgments::JudgmentConfig {
                enabled: true,
                provider: "typesafe".into(),
                base_url: "http://127.0.0.1:1".into(),
                model: "fake".into(),
                choice_min: 0.8,
                wake_yes: 0.8,
                similar_requirements: None,
                credential_source: "file".into(),
            }),
        };
        crewd_store::meta_set(
            s.conn(),
            crate::judgments::CONFIG_KEY,
            &serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='w-a1'", [])
            .unwrap();
        let mut previous = None;
        for _ in 0..2 {
            let parent = match edge {
                Some(crate::admin_case_link::Edge::Parent) => previous.clone(),
                _ => None,
            };
            let depends_on: Vec<String> = match edge {
                Some(crate::admin_case_link::Edge::Dependency) => previous.clone().into_iter().collect(),
                _ => Vec::new(),
            };
            let next_check_at = (!depends_on.is_empty()).then(|| crewd_store::ts_from_ms(NOW));
            let row = ok(s, |tx| {
                work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: "crew-a".into(),
                        owner: Some("w-a1".into()),
                        title: "synthetic case work".into(),
                        parent,
                        depends_on,
                        next_check_at,
                        ..Default::default()
                    },
                )
            });
            previous = Some(row.id);
        }
        for _ in 0..4 {
            s.conn()
                .execute(
                    "UPDATE wakes SET state='delivered' WHERE session_id='w-a1'",
                    [],
                )
                .unwrap();
            let c = stop_checks::candidate(s.conn(), "w-a1", NOW)
                .unwrap()
                .unwrap();
            ok(s, |tx| stop_checks::accept(tx, &c));
        }
        let source = admin_cases::list_at(
            s.conn(),
            &AdminCaseListParams {
                crew_id: "crew-a".into(),
                after: None,
                limit: None,
            },
            NOW,
        )
        .unwrap()
        .items
        .remove(0);
        let decision = ok(s, |tx| admin_cases::prepare(tx, "crew-a", &source.id)).unwrap();
        assert!(ok(s, |tx| admin_cases::dispatch(tx, &decision)));
        assert!(ok(s, |tx| admin_cases::settle(
            tx,
            &decision,
            Some(0.99),
            AdminCaseDecisionReason::NativeAnswered
        )));
        source
    }
    #[test]
    fn adopted_current_case_yields_typed_launch_permit() {
        let mut s = store();
        let source = adopted(&mut s);
        assert!(ok(&mut s, |tx| launch_permit(tx, "crew-a", &source.id)).is_some());
    }
    pub(crate) fn running_case(s: &mut crewd_store::Store) -> (AdminCaseView, Registration) {
        running_case_with_edge(s, None)
    }
    pub(crate) fn running_case_with_edge(s: &mut crewd_store::Store, edge: Option<crate::admin_case_link::Edge>) -> (AdminCaseView, Registration) {
        let case = adopted_with_edge(s, edge);
        let permit = ok(s, |tx| launch_permit(tx, "crew-a", &case.id)).unwrap();
        let registered = ok(s, |tx| register_and_bind(tx, &permit));
        assert!(ok(s, |tx| claim_launch(tx, &registered.witness)));
        s.conn()
            .execute(
                "UPDATE sessions SET desired='running' WHERE id=?1",
                [&registered.session.id],
            )
            .unwrap();
        assert!(ok(s, |tx| confirm_started(
            tx,
            &registered.witness,
            "1".into()
        )));
        (case, registered)
    }
    #[test]
    fn native_lease_source_stop_must_revoke_physical_authority() {
        let mut s = store();
        let (_, reg) = running_case(&mut s);
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Active(_)
        ));
        s.conn()
            .execute("UPDATE sessions SET desired='stopped' WHERE id='w-a1'", [])
            .unwrap();
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Revoked(_, NativeRevocationReason::AuthorityChanged)
        ));
    }
    #[test]
    fn native_profile_first_observation_preserves_native_authority() {
        let mut s = store();
        let (_, reg) = running_case(&mut s);
        let row = crewd_store::sessions::get(s.conn(), &reg.session.id)
            .unwrap()
            .unwrap();
        assert!(row.model.is_none() && row.effort.is_none());
        assert!(ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("actual-native-model".into()),
            Some("high".into())
        )));
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Active(_)
        ));
    }
    #[test]
    fn native_profile_repeat_is_idempotent_and_conflicting_values_do_not_write() {
        let mut s = store();
        let (_, reg) = running_case(&mut s);
        assert!(ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("native-model".into()),
            Some("high".into())
        )));
        let raw = meta_get(s.conn(), &key(reg.witness.case_id())).unwrap();
        assert!(ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("native-model".into()),
            Some("high".into())
        )));
        assert_eq!(
            meta_get(s.conn(), &key(reg.witness.case_id())).unwrap(),
            raw
        );
        assert!(!ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("changed-model".into()),
            None
        )));
        assert!(!ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            2,
            None,
            Some("low".into())
        )));
        assert_eq!(
            meta_get(s.conn(), &key(reg.witness.case_id())).unwrap(),
            raw
        );
        let row = crewd_store::sessions::get(s.conn(), &reg.session.id)
            .unwrap()
            .unwrap();
        assert_eq!(row.model.as_deref(), Some("native-model"));
        assert_eq!(row.effort.as_deref(), Some("high"));
    }
    #[test]
    fn native_profile_does_not_reauthorize_revoked_captain_or_external_profile_changes() {
        for mode in 0..3 {
            let mut s = store();
            let (_, reg) = running_case(&mut s);
            match mode {
                0 => {
                    let NativeLeaseState::Active(lease) =
                        native_lease(s.conn(), &reg.witness, NOW).unwrap()
                    else {
                        panic!("active")
                    };
                    ok(&mut s, |tx| {
                        revoke_native(tx, &lease, NativeRevocationReason::AdminAdmissionLost)
                    });
                }
                1 => {
                    s.conn()
                        .execute(
                            "UPDATE crews SET captain_session_id=NULL WHERE id='crew-a'",
                            [],
                        )
                        .unwrap();
                }
                _ => {
                    s.conn()
                        .execute(
                            "UPDATE sessions SET model='external-change' WHERE id=?1",
                            [&reg.session.id],
                        )
                        .unwrap();
                }
            }
            let raw = meta_get(s.conn(), &key(reg.witness.case_id())).unwrap();
            assert!(!ok(&mut s, |tx| observe_native_profile(
                tx,
                &reg.witness,
                1,
                Some("native-model".into()),
                Some("high".into())
            )));
            assert_eq!(
                meta_get(s.conn(), &key(reg.witness.case_id())).unwrap(),
                raw
            );
            let row = crewd_store::sessions::get(s.conn(), &reg.session.id)
                .unwrap()
                .unwrap();
            assert_eq!(row.effort, None);
        }
    }
    #[test]
    fn native_profile_must_match_explicit_inherited_model_and_effort() {
        let mut s = store();
        s.conn()
            .execute(
                "UPDATE sessions SET model='explicit-model', effort='high' WHERE id='w-a1'",
                [],
            )
            .unwrap();
        let (_, reg) = running_case(&mut s);
        assert!(!ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("different-model".into()),
            Some("high".into())
        )));
        assert!(!ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("explicit-model".into()),
            Some("low".into())
        )));
        assert!(ok(&mut s, |tx| observe_native_profile(
            tx,
            &reg.witness,
            1,
            Some("explicit-model".into()),
            Some("high".into())
        )));
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Active(_)
        ));
    }
    #[test]
    fn native_profile_row_and_context_rollback_together() {
        let mut s = store();
        let (_, reg) = running_case(&mut s);
        let raw = meta_get(s.conn(), &key(reg.witness.case_id())).unwrap();
        assert!(
            crate::run::<()>(&mut s, NOW, |tx| {
                assert!(observe_native_profile(
                    tx,
                    &reg.witness,
                    1,
                    Some("native-model".into()),
                    Some("high".into())
                )?);
                Err(invalid())
            })
            .is_err()
        );
        assert_eq!(
            meta_get(s.conn(), &key(reg.witness.case_id())).unwrap(),
            raw
        );
        let row = crewd_store::sessions::get(s.conn(), &reg.session.id)
            .unwrap()
            .unwrap();
        assert!(row.model.is_none() && row.effort.is_none());
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Active(_)
        ));
    }
    #[test]
    fn native_lease_revocation_is_durable_and_preserves_attempt_generation() {
        let mut s = store();
        let (_, reg) = running_case(&mut s);
        let NativeLeaseState::Active(lease) = native_lease(s.conn(), &reg.witness, NOW).unwrap()
        else {
            panic!("active")
        };
        assert!(ok(&mut s, |tx| revoke_native(
            tx,
            &lease,
            NativeRevocationReason::SourceAdmissionLost
        )));
        assert!(!ok(&mut s, |tx| revoke_native(
            tx,
            &lease,
            NativeRevocationReason::SourceAdmissionLost
        )));
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Revoked(_, NativeRevocationReason::SourceAdmissionLost)
        ));
        let saved = record(s.conn(), reg.witness.case_id()).unwrap().unwrap();
        assert_eq!(saved.view.phase, AdminCaseExecution::Unknown);
        assert_eq!(saved.view.generation.as_deref(), Some("1"));
        assert_eq!(saved.view.attempt_id, reg.witness.attempt_id());
        assert!(
            binding(s.conn(), "crew-a", reg.witness.case_id())
                .unwrap()
                .is_some()
        );
        let raw = meta_get(s.conn(), &key(reg.witness.case_id()))
            .unwrap()
            .unwrap();
        let mut tampered: serde_json::Value = serde_json::from_str(&raw).unwrap();
        tampered["native_revocation"]["reason"] = "admin_admission_lost".into();
        meta_set(s.conn(), &key(reg.witness.case_id()), &tampered.to_string()).unwrap();
        assert!(native_lease(s.conn(), &reg.witness, NOW).is_err());
    }
    #[test]
    fn native_lease_old_records_and_captain_changes_fail_closed() {
        let mut s = store();
        let (_, reg) = running_case(&mut s);
        s.conn()
            .execute(
                "UPDATE crews SET captain_session_id=NULL WHERE id='crew-a'",
                [],
            )
            .unwrap();
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Revoked(_, NativeRevocationReason::CaptainChanged)
        ));
        let mut saved = record(s.conn(), reg.witness.case_id()).unwrap().unwrap();
        saved.native_captain = None;
        ok(&mut s, |tx| save(tx, &saved));
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Revoked(_, NativeRevocationReason::MissingWitness)
        ));
    }
    #[test]
    fn native_lease_legal_effects_refresh_but_external_changes_revoke() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        ok(&mut s, |tx| {
            let p = validate_effect(
                tx,
                &reg.witness,
                ActionScope::WorkClose {
                    id: case.targets[0].id.clone(),
                    kind: WorkClosureKind::Done,
                },
            )?;
            close(tx, &reg.session.id, &case.targets[0].id)?;
            finish_effect(tx, p)
        });
        let NativeLeaseState::Active(lease) = native_lease(s.conn(), &reg.witness, NOW).unwrap()
        else {
            panic!("legal effect")
        };
        assert!(!ok(&mut s, |tx| revoke_native(
            tx,
            &lease,
            NativeRevocationReason::TargetChanged
        )));
        ok(&mut s, |tx| close(tx, "cap-a", &case.targets[1].id));
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Revoked(_, NativeRevocationReason::TargetChanged)
        ));
        ok(&mut s, |tx| {
            crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
        });
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Released(_, NativeReleaseReason::Resolved)
        ));
        assert!(
            validate_read(
                s.conn(),
                &reg.witness,
                ReadScope::Work {
                    id: case.targets[0].id.clone()
                },
                NOW
            )
            .is_ok()
        );
        ok(&mut s, |tx| {
            assert!(
                validate_effect(
                    tx,
                    &reg.witness,
                    ActionScope::WorkClose {
                        id: case.targets[0].id.clone(),
                        kind: WorkClosureKind::Done
                    }
                )
                .is_err()
            );
            Ok(())
        });
    }
    #[test]
    fn native_lease_starting_without_generation_and_revocation_rollback() {
        let mut s = store();
        let case = adopted(&mut s);
        let p = ok(&mut s, |tx| launch_permit(tx, "crew-a", &case.id)).unwrap();
        let reg = ok(&mut s, |tx| register_and_bind(tx, &p));
        ok(&mut s, |tx| claim_launch(tx, &reg.witness));
        let NativeLeaseState::Active(lease) = native_lease(s.conn(), &reg.witness, NOW).unwrap()
        else {
            panic!("starting")
        };
        assert_eq!(lease.generation(), None);
        assert!(
            crate::run::<()>(&mut s, NOW, |tx| {
                revoke_native(tx, &lease, NativeRevocationReason::AuthorityUnknown)?;
                Err(invalid())
            })
            .is_err()
        );
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Active(_)
        ));
        assert!(ok(&mut s, |tx| revoke_native(
            tx,
            &lease,
            NativeRevocationReason::AuthorityUnknown
        )));
        assert!(!ok(&mut s, |tx| confirm_started(
            tx,
            &reg.witness,
            "1".into()
        )));
    }
    #[test]
    fn native_lease_configuration_and_restore_revoke_even_released_work() {
        for restore in [false, true] {
            let mut s = store();
            let (case, reg) = running_case(&mut s);
            for target in &case.targets {
                ok(&mut s, |tx| close(tx, "cap-a", &target.id));
            }
            ok(&mut s, |tx| {
                crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
            });
            assert!(matches!(
                native_lease(s.conn(), &reg.witness, NOW).unwrap(),
                NativeLeaseState::Released(_, _)
            ));
            if restore {
                crewd_store::judgment_state::restored(s.conn()).unwrap();
            } else {
                let mut settings = judgments::configuration(s.conn()).unwrap();
                settings.revision = Some("changed-native-lease".into());
                meta_set(
                    s.conn(),
                    judgments::CONFIG_KEY,
                    &serde_json::to_string(&settings).unwrap(),
                )
                .unwrap();
            }
            assert!(matches!(
                native_lease(s.conn(), &reg.witness, NOW).unwrap(),
                NativeLeaseState::Revoked(_, NativeRevocationReason::AuthorityChanged)
            ));
        }
    }
    #[test]
    fn native_lease_accepted_handoff_allows_captain_progress_not_admin_writes() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        let id = uuid::Uuid::new_v4().to_string();
        let v = ok(&mut s, |tx| {
            crate::admin_case_handoff::request(
                tx,
                &Sender::Session(reg.session.id.clone()),
                &id,
                "need custody",
            )
        });
        ok(&mut s, |tx| {
            crate::wakes::claim(tx, &[v.wake_id])?;
            crate::wakes::ack_delivered(
                tx,
                &v.captain_session_id,
                &[v.wake_id],
                crate::wakes::Riders::Release,
                &[],
                &[],
            )
        });
        let v = crate::admin_case_handoff::get(
            s.conn(),
            &Sender::Session(v.captain_session_id.clone()),
            &id,
            NOW,
        )
        .unwrap();
        ok(&mut s, |tx| {
            crate::admin_case_handoff::accept(
                tx,
                &Sender::Session(v.captain_session_id.clone()),
                &id,
                &v.revision,
                "accept custody",
            )
        });
        ok(&mut s, |tx| close(tx, "cap-a", &case.targets[0].id));
        assert!(matches!(
            native_lease(s.conn(), &reg.witness, NOW).unwrap(),
            NativeLeaseState::Released(_, NativeReleaseReason::HandoffAccepted)
        ));
        ok(&mut s, |tx| {
            assert!(
                validate_effect(
                    tx,
                    &reg.witness,
                    ActionScope::WorkClose {
                        id: case.targets[1].id.clone(),
                        kind: WorkClosureKind::Done
                    }
                )
                .is_err()
            );
            Ok(())
        });
        assert!(
            validate_read(
                s.conn(),
                &reg.witness,
                ReadScope::Work {
                    id: case.targets[0].id.clone()
                },
                NOW
            )
            .is_ok()
        );
    }
    pub(crate) fn close(tx: &mut Tx<'_>, session: &str, id: &str) -> Result<WorkPlan> {
        let plan = work::get(tx.conn(), id)?;
        work::close_factual(
            tx,
            &Sender::Session(session.into()),
            WorkCloseParams {
                id: id.into(),
                expected_revision: plan.revision,
                kind: WorkClosureKind::Done,
                evidence: WorkEvidenceParam {
                    kind: WorkEvidenceKind::Text,
                    value: "synthetic checked artifact".into(),
                },
                note: "synthetic factual closure".into(),
            },
        )
    }
    #[test]
    fn unique_registration_inherits_source_and_recovery_never_creates_second_session() {
        let mut s = store();
        let case = adopted(&mut s);
        let permit = ok(&mut s, |tx| launch_permit(tx, "crew-a", &case.id)).unwrap();
        let reg = ok(&mut s, |tx| register_and_bind(tx, &permit));
        assert_eq!(reg.session.role, Role::Admin);
        assert_eq!(reg.session.crew_id, "crew-a");
        let source = crewd_store::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .unwrap();
        assert_eq!(reg.session.executor, source.executor);
        assert_eq!(reg.session.model, source.model);
        let count: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert!(crate::run(&mut s, NOW, |tx| register_and_bind(tx, &permit)).is_err());
        assert!(ok(&mut s, |tx| claim_launch(tx, &reg.witness)));
        assert!(!ok(&mut s, |tx| claim_launch(tx, &reg.witness)));
        ok(&mut s, |tx| recover_page(tx, None));
        assert_eq!(
            record(s.conn(), &case.id).unwrap().unwrap().view.phase,
            AdminCaseExecution::Unknown
        );
        assert!(ok(&mut s, |tx| launch_permit(tx, "crew-a", &case.id)).is_none());
        assert!(!ok(&mut s, |tx| claim_launch(tx, &reg.witness)));
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
        assert_eq!(
            binding(s.conn(), "crew-a", &case.id)
                .unwrap()
                .unwrap()
                .session_id(),
            reg.session.id
        );
    }
    #[test]
    fn two_typed_closures_advance_checkpoint_but_never_claim_case_resolution() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        for target in &case.targets {
            assert!(
                validate_read(
                    s.conn(),
                    &reg.witness,
                    ReadScope::Work {
                        id: target.id.clone()
                    },
                    NOW
                )
                .is_ok()
            );
            ok(&mut s, |tx| {
                let permit = validate_effect(
                    tx,
                    &reg.witness,
                    ActionScope::WorkClose {
                        id: target.id.clone(),
                        kind: WorkClosureKind::Done,
                    },
                )?;
                close(tx, &reg.session.id, &target.id)?;
                finish_effect(tx, permit)
            });
            assert!(
                validate_read(
                    s.conn(),
                    &reg.witness,
                    ReadScope::Work {
                        id: target.id.clone()
                    },
                    NOW
                )
                .is_ok()
            );
        }
        let result = admin_cases::get_at(
            s.conn(),
            &AdminCaseGetParams {
                crew_id: "crew-a".into(),
                id: case.id.clone(),
            },
            NOW,
        )
        .unwrap();
        assert_eq!(result.execution, AdminCaseExecution::Running);
        assert_eq!(result.execution_record.unwrap().effects_count, 2);
        assert_eq!(result.retirement, AdminCaseRetirement::NotStarted);
    }
    #[test]
    fn missing_or_extra_effect_rolls_back_real_work_changes() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        let action = ActionScope::WorkClose {
            id: case.targets[0].id.clone(),
            kind: WorkClosureKind::Done,
        };
        assert!(
            crate::run(&mut s, NOW, |tx| {
                let p = validate_effect(tx, &reg.witness, action.clone())?;
                finish_effect(tx, p)
            })
            .is_err()
        );
        assert!(
            crate::run(&mut s, NOW, |tx| {
                let p = validate_effect(tx, &reg.witness, action.clone())?;
                close(tx, &reg.session.id, &case.targets[0].id)?;
                close(tx, &reg.session.id, &case.targets[1].id)?;
                finish_effect(tx, p)
            })
            .is_err()
        );
        for target in &case.targets {
            assert_ne!(work::get(s.conn(), &target.id).unwrap().status, "done");
        }
        assert_eq!(
            record(s.conn(), &case.id)
                .unwrap()
                .unwrap()
                .view
                .effects_count,
            0
        );
    }
    #[test]
    fn foreign_target_restore_and_role_change_reject_execution() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        let other = ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-b".into(),
                    title: "foreign".into(),
                    ..Default::default()
                },
            )
        });
        assert!(
            validate_read(
                s.conn(),
                &reg.witness,
                ReadScope::Work { id: other.id },
                NOW
            )
            .is_err()
        );
        s.conn()
            .execute(
                "UPDATE sessions SET role='worker' WHERE id=?1",
                [&reg.session.id],
            )
            .unwrap();
        assert!(
            validate_read(
                s.conn(),
                &reg.witness,
                ReadScope::Work {
                    id: case.targets[0].id.clone()
                },
                NOW
            )
            .is_err()
        );
        s.conn()
            .execute(
                "UPDATE sessions SET role='admin' WHERE id=?1",
                [&reg.session.id],
            )
            .unwrap();
        ok(&mut s, |tx| {
            crewd_store::judgment_state::restored(tx.conn())?;
            Ok(())
        });
        assert!(
            validate_read(
                s.conn(),
                &reg.witness,
                ReadScope::Work {
                    id: case.targets[0].id.clone()
                },
                NOW
            )
            .is_err()
        );
    }
    #[test]
    fn registration_binding_failure_rolls_back_session_and_can_retry_once() {
        let mut s = store();
        let case = adopted(&mut s);
        let permit = ok(&mut s, |tx| launch_permit(tx, "crew-a", &case.id)).unwrap();
        let before: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        s.conn().execute_batch("CREATE TRIGGER fail_case_bind BEFORE INSERT ON meta WHEN NEW.key LIKE 'admin.case.execution.v1:%' BEGIN SELECT RAISE(ABORT,'synthetic binding failure'); END").unwrap();
        assert!(crate::run(&mut s, NOW, |tx| register_and_bind(tx, &permit)).is_err());
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        assert!(binding(s.conn(), "crew-a", &case.id).unwrap().is_none());
        s.conn()
            .execute_batch("DROP TRIGGER fail_case_bind")
            .unwrap();
        let registration = ok(&mut s, |tx| register_and_bind(tx, &permit));
        assert!(ok(&mut s, |tx| mark_unknown(tx, &registration.witness)));
        assert_eq!(
            record(s.conn(), &case.id).unwrap().unwrap().view.phase,
            AdminCaseExecution::Unknown
        );
        assert!(!ok(&mut s, |tx| claim_launch(tx, &registration.witness)));
    }
    #[test]
    fn missing_both_identity_hints_still_find_durable_case_without_blocking_others() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [session_key(&reg.session.id)],
            )
            .unwrap();
        s.conn()
            .execute(
                "UPDATE sessions SET extra=json_remove(extra,'$.adminCaseExecutionV1') WHERE id=?1",
                [&reg.session.id],
            )
            .unwrap();
        assert!(session_binding(s.conn(), &reg.session.id).is_err());
        assert!(session_binding(s.conn(), "w-a1").unwrap().is_none());
        meta_set(
            s.conn(),
            "admin.case.execution.v1:unrelated-corrupt",
            "not json",
        )
        .unwrap();
        assert!(session_binding(s.conn(), "w-a1").unwrap().is_none());
        assert!(session_binding(s.conn(), &reg.session.id).is_err());
        // JSON duplicate-key selection must not hide a retained identity witness.
        let raw = format!(
            "{{\"view\":{{\"sessionId\":\"other\"}},\"view\":{{\"sessionId\":{}}}}}",
            serde_json::to_string(&reg.session.id).unwrap()
        );
        meta_set(s.conn(), &key(&case.id), &raw).unwrap();
        assert!(session_binding(s.conn(), &reg.session.id).is_err());
        assert!(session_binding(s.conn(), "w-a1").unwrap().is_none());
    }
    #[test]
    fn missing_reverse_index_cannot_reclassify_bound_admin_as_ordinary() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        assert!(
            session_binding(s.conn(), &reg.session.id)
                .unwrap()
                .is_some()
        );
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [session_key(&reg.session.id)],
            )
            .unwrap();
        assert!(session_binding(s.conn(), &reg.session.id).is_err());
        assert!(binding(s.conn(), "crew-a", &case.id).is_err());
        assert!(session_binding(s.conn(), "w-a1").unwrap().is_none());
    }
    #[test]
    fn verified_silent_owner_reassignment_records_real_owner_change() {
        use crate::testkit::ok_at;
        let mut s = store();
        let case = adopted(&mut s);
        let id = case.targets[0].id.clone();
        let reference = crewd_store::object_refs::get(
            s.conn(),
            crewd_store::object_refs::ObjectKind::Plan,
            work::number(&id).unwrap(),
        )
        .unwrap()
        .unwrap();
        let core = work::core_plan(s.conn(), &reference).unwrap();
        ok(&mut s, |tx| {
            let p = work::get(tx.conn(), &id)?;
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: id.clone(),
                    expected_revision: p.revision,
                    status: Some(WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 60_000)),
                    note: Some("synthetic progress".into()),
                    ..Default::default()
                },
            )
        });
        let checks = crewd_work::Config::default().missed_checks_limit;
        for _ in 0..checks {
            ok(&mut s, |tx| {
                crate::asks::with_book(tx, "crew-a", |book, now| {
                    book.record_checkpoint_wake(
                        &crewd_work::Actor::System,
                        now,
                        &crewd_work::SessionId::new("w-a1"),
                        &[core],
                    )
                    .map(|_| ())
                    .map_err(Into::into)
                })
            });
        }
        let now = crate::asks::load_org(s.conn())
            .unwrap()
            .item(core)
            .unwrap()
            .next_check_at
            .unwrap()
            .0;
        let decision = ok_at(&mut s, now, |tx| {
            admin_cases::prepare(tx, "crew-a", &case.id)
        })
        .0
        .unwrap();
        assert!(ok_at(&mut s, now, |tx| admin_cases::dispatch(tx, &decision)).0);
        assert!(
            ok_at(&mut s, now, |tx| admin_cases::settle(
                tx,
                &decision,
                Some(0.99),
                AdminCaseDecisionReason::NativeAnswered
            ))
            .0
        );
        let permit = ok_at(&mut s, now, |tx| launch_permit(tx, "crew-a", &case.id))
            .0
            .unwrap();
        let reg = ok_at(&mut s, now, |tx| register_and_bind(tx, &permit)).0;
        assert!(ok_at(&mut s, now, |tx| claim_launch(tx, &reg.witness)).0);
        s.conn()
            .execute(
                "UPDATE sessions SET desired='running' WHERE id=?1",
                [&reg.session.id],
            )
            .unwrap();
        assert!(
            ok_at(&mut s, now, |tx| confirm_started(
                tx,
                &reg.witness,
                "2".into()
            ))
            .0
        );
        ok_at(&mut s, now, |tx| {
            let effect = validate_effect(
                tx,
                &reg.witness,
                ActionScope::WorkReassignLost {
                    id: id.clone(),
                    new_owner: "w-a2".into(),
                },
            )?;
            let current = work::get(tx.conn(), &id)?;
            crate::work_lost::reassign(
                tx,
                &Sender::Session(reg.session.id.clone()),
                WorkAssignParams {
                    id: id.clone(),
                    expected_revision: current.revision,
                    owner: "w-a2".into(),
                    note: "verified silent owner".into(),
                },
            )?;
            finish_effect(tx, effect)
        });
        assert_eq!(
            work::get(s.conn(), &id).unwrap().owner.as_deref(),
            Some("w-a2")
        );
        let page = crate::admin_case_effect_report::list(s.conn(), &AdminCaseEffectsParams {
            crew_id: "crew-a".into(), id: case.id.clone(), after: None, limit: None,
        }).unwrap();
        let [effect] = page.items.as_slice() else { panic!("one reassignment effect") };
        assert_eq!(effect.work_id, id);
        assert_eq!(effect.occurred_at_ms, now);
        let AdminCaseEffectAction::Reassigned { previous_owner, new_owner, note, wake_id } = &effect.action
        else { panic!("genuine lost owner change") };
        assert_eq!(previous_owner.as_deref(), Some("w-a1"));
        assert_eq!(new_owner, "w-a2");
        assert_eq!(note, "verified silent owner");
        assert!(wake_id.is_some());
        assert_eq!(
            record(s.conn(), &case.id)
                .unwrap()
                .unwrap()
                .view
                .effects_count,
            1
        );
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectReceipt {
    pub case_id: String,
    pub session_id: String,
    pub work_id: String,
    pub first_event: u64,
    pub last_event: u64,
    pub checkpoint: String,
    #[serde(default)]
    pub result_revision: Option<String>,
    #[serde(default)]
    pub wake_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_request_id: Option<String>,
}

pub(crate) struct EffectHistory {
    pub witness: ExecutionWitness,
    pub targets: Vec<String>,
    pub count: u64,
}

fn effect_prefix(attempt: &str) -> String {
    format!("admin.case.effect.v1:{attempt}:")
}

fn effect_key(attempt: &str, ordinal: u64) -> String {
    format!("{}{ordinal:020}", effect_prefix(attempt))
}

impl EffectHistory {
    pub fn receipt(&self, c: &Connection, ordinal: u64) -> Result<EffectReceipt> {
        let key = effect_key(&self.witness.attempt, ordinal);
        let raw = meta_get(c, &key)?.ok_or_else(invalid)?;
        decode_effect(&raw, &self.witness, &self.targets)
    }
}

pub(crate) fn effect_history(c: &Connection, crew: &str, id: &str) -> Result<Option<EffectHistory>> {
    let Some(r) = record(c, id)? else {
        return Ok(None);
    };
    if r.crew != crew {
        return Err(invalid());
    }
    let prefix = effect_prefix(&r.view.attempt_id);
    let last_key: Option<String> = c.query_row(
        "SELECT key FROM meta WHERE key>=?1 AND key<?2 ORDER BY key DESC LIMIT 1",
        params![prefix, format!("{prefix}\u{10ffff}")],
        |row| row.get(0),
    ).optional()?;
    let expected = (r.view.effects_count > 0)
        .then(|| effect_key(&r.view.attempt_id, r.view.effects_count));
    if last_key != expected {
        return Err(invalid());
    }
    Ok(Some(EffectHistory {
        witness: witness(&r),
        targets: r.targets,
        count: r.view.effects_count,
    }))
}

fn decode_effect(raw: &str, w: &ExecutionWitness, targets: &[String]) -> Result<EffectReceipt> {
    let e: EffectReceipt = serde_json::from_str(raw).map_err(StoreError::from)?;
    if e.case_id != w.case
        || e.session_id != w.session
        || !targets.contains(&e.work_id)
        || e.first_event == 0
        || e.last_event < e.first_event
        || e.last_event > i64::MAX as u64
        || e.last_event - e.first_event >= 256
    {
        return Err(invalid());
    }
    Ok(e)
}
pub(crate) struct OutcomeAuthority {
    pub actually_started: bool,
    pub current: bool,
    pub fingerprint: String,
}
pub(crate) fn outcome_authority(c: &Connection, w: &ExecutionWitness) -> Result<OutcomeAuthority> {
    let r = matching(c, w)?;
    let environment = environment_for(c, &r, false).ok();
    let current = environment.as_deref() == Some(r.context.as_str());
    let fingerprint = judgments::fingerprint(&(
        &r.context,
        &environment,
        &r.source_revision,
        &r.decision,
        &r.targets,
    ))?;
    Ok(OutcomeAuthority {
        actually_started: r.view.generation.is_some(),
        current,
        fingerprint,
    })
}
pub(crate) fn effect_receipts(c: &Connection, w: &ExecutionWitness) -> Result<Vec<EffectReceipt>> {
    let r = matching(c, w)?;
    let prefix = effect_prefix(&r.view.attempt_id);
    let mut q = c.prepare("SELECT key,value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")?;
    let rows = q
        .query_map(params![prefix, format!("{prefix}\u{10ffff}")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (key, raw) in rows {
        let ordinal = key
            .strip_prefix(&prefix)
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| *n > 0 && *n <= r.view.effects_count)
            .ok_or_else(invalid)?;
        if key != effect_key(&r.view.attempt_id, ordinal) {
            return Err(invalid());
        }
        out.push(decode_effect(&raw, w, &r.targets)?);
    }
    Ok(out)
}

/// The persisted Core launch receipt, not a client projection or actor supplied string.
pub(crate) fn generation(c: &Connection, w: &ExecutionWitness) -> Result<Option<u64>> {
    let r = matching(c, w)?;
    r.view
        .generation
        .map(|value| {
            let n = value.parse::<u64>().map_err(|_| invalid())?;
            if n.to_string() != value {
                return Err(invalid());
            }
            Ok(n)
        })
        .transpose()
}

/// Internal bounded communication authority, using the same current checkpoint as effects.
pub(crate) fn handoff_fingerprint(
    c: &Connection,
    w: &ExecutionWitness,
    now: i64,
) -> Result<String> {
    let r = running(c, w, now)?;
    judgments::fingerprint(&(&r.checkpoint, &r.context, &r.view.attempt_id))
}
