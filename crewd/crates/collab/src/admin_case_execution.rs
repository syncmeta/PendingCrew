//! One bounded administrative executor per immutable case; never Human authority.
use crate::{Result, Tx, admin_cases, judgments};
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug)]
pub struct AdminLaunchPermit {
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
#[derive(Clone, Debug)]
pub enum ReadScope {
    Work { id: String },
    SourceSession { id: String },
}
#[derive(Clone, Debug)]
pub enum ActionScope {
    WorkClose { id: String, kind: WorkClosureKind },
    WorkReassignLost { id: String, new_owner: String },
}
impl ActionScope {
    fn id(&self) -> &str {
        match self {
            Self::WorkClose { id, .. } | Self::WorkReassignLost { id, .. } => id,
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
        (None, None) => Ok(None),
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
            title: Some("行政事项".into()),
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
    let r = running(c, w, now)?;
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
    let before_seq = crate::asks::load_org(tx.conn())?.state().last_seq;
    let before_revision = crate::work::get(tx.conn(), scope.id())?.revision;
    Ok(EffectPermit {
        witness: w.clone(),
        action: scope,
        core,
        before_seq,
        before_revision,
        checkpoint: r.checkpoint,
        context: r.context,
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
    let mut primary = 0;
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
            ) if *id == permit.core => {
                primary += 1;
                true
            }
            (ActionScope::WorkClose { kind, .. }, E::Dropped { id, reason })
                if *id == permit.core && reason.evidence.is_some() =>
            {
                let same = matches!(
                    (kind, reason.kind),
                    (WorkClosureKind::Duplicate, crewd_work::DropKind::Duplicate)
                        | (
                            WorkClosureKind::Superseded,
                            crewd_work::DropKind::Superseded
                        )
                );
                if same {
                    primary += 1;
                }
                same
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
            ) if *id == permit.core && to.as_str() == new_owner => {
                primary += 1;
                true
            }
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    if primary != 1 || crate::asks::load_org(tx.conn())?.state().last_seq != expected {
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
    };
    meta_set(
        tx.conn(),
        &format!(
            "admin.case.effect.v1:{}:{:020}",
            r.view.attempt_id, r.view.effects_count
        ),
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
    fn adopted(s: &mut crewd_store::Store) -> AdminCaseView {
        let settings = crewd_proto::judgments::JudgmentSettings {
            revision: Some("synthetic".into()),
            config: Some(crewd_proto::judgments::JudgmentConfig {
                enabled: true,
                provider: "typesafe".into(),
                base_url: "http://127.0.0.1:1".into(),
                model: "fake".into(),
                choice_min: 0.8,
                wake_yes: 0.8,
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
        for _ in 0..2 {
            ok(s, |tx| {
                work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: "crew-a".into(),
                        owner: Some("w-a1".into()),
                        title: "synthetic case work".into(),
                        ..Default::default()
                    },
                )
            });
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
        let case = adopted(s);
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
    let prefix = format!("admin.case.effect.v1:{}:", r.view.attempt_id);
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
        if key != format!("{prefix}{ordinal:020}") {
            return Err(invalid());
        }
        let e: EffectReceipt = serde_json::from_str(&raw).map_err(StoreError::from)?;
        if e.case_id != r.case
            || e.session_id != r.view.session_id
            || !r.targets.contains(&e.work_id)
            || e.first_event == 0
            || e.last_event < e.first_event
            || e.last_event - e.first_event >= 256
        {
            return Err(invalid());
        }
        out.push(e);
    }
    Ok(out)
}
