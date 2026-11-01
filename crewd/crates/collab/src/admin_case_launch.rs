//! Durable scheduling intent. Execution remains the sole launch/binding authority.
use crate::{
    Result, Tx,
    admin_case_execution::{self, AdminLaunchPermit, ExecutionWitness},
    admin_cases, judgments,
};
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    crew: String,
    case: String,
    fingerprint: String,
    view: AdminCaseLaunchView,
    checksum: String,
}
fn key(id: &str) -> String {
    format!("admin.case.launch.v1:{id}")
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid administrative launch schedule".into()).into()
}
fn digest(r: &Saved) -> Result<String> {
    judgments::fingerprint(&(r.version, &r.crew, &r.case, &r.fingerprint, &r.view))
}
fn read(c: &Connection, id: &str) -> Result<Option<Saved>> {
    let Some(raw) = meta_get(c, &key(id))? else {
        return Ok(None);
    };
    let r: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if r.version != 1
        || r.case != id
        || r.crew.is_empty()
        || r.fingerprint.is_empty()
        || r.view.revision.is_empty()
        || r.checksum != digest(&r)?
    {
        return Err(invalid());
    }
    Ok(Some(r))
}
fn save(tx: &mut Tx<'_>, r: &mut Saved) -> Result<()> {
    r.checksum = digest(r)?;
    meta_set(
        tx.conn(),
        &key(&r.case),
        &serde_json::to_string(r).map_err(StoreError::from)?,
    )?;
    Ok(())
}
fn current(c: &Connection, r: &Saved, now: i64) -> Result<bool> {
    Ok(
        admin_cases::execution_eligibility(c, &r.crew, &r.case, now)?
            .is_some_and(|(s, d)| s.fingerprint == r.fingerprint && d == r.view.decision_id),
    )
}
/// No provider invocation. Only an adopted, current native decision can create intent.
pub fn prepare(tx: &mut Tx<'_>, crew: &str, id: &str) -> Result<Option<AdminLaunchPermit>> {
    let old = read(tx.conn(), id)?;
    if old.as_ref().is_some_and(|r| r.crew != crew) {
        return Err(invalid());
    }
    if admin_case_execution::binding(tx.conn(), crew, id)?.is_some() {
        return Ok(None);
    }
    if old.as_ref().is_some_and(|r| {
        matches!(
            r.view.phase,
            AdminCaseLaunchPhase::Dispatching
                | AdminCaseLaunchPhase::Unknown
                | AdminCaseLaunchPhase::Bound
        )
    }) {
        return Ok(None);
    }
    let Some(mut permit) = admin_case_execution::launch_permit(tx, crew, id)? else {
        return Ok(None);
    };
    let Some((snapshot, decision)) =
        admin_cases::execution_eligibility(tx.conn(), crew, id, tx.now_ms())?
    else {
        return Ok(None);
    };
    let r = match old {
        Some(r) if r.fingerprint == snapshot.fingerprint && r.view.decision_id == decision => r,
        _ => {
            let mut r = Saved {
                version: 1,
                crew: crew.into(),
                case: id.into(),
                fingerprint: snapshot.fingerprint,
                view: AdminCaseLaunchView {
                    phase: AdminCaseLaunchPhase::Waiting,
                    reason: AdminCaseLaunchReason::Ready,
                    revision: uuid::Uuid::new_v4().to_string(),
                    decision_id: decision,
                    updated_at: tx.now_ms(),
                    attempt_id: None,
                    session_id: None,
                },
                checksum: String::new(),
            };
            save(tx, &mut r)?;
            r
        }
    };
    permit.schedule_revision = Some(r.view.revision);
    Ok(Some(permit))
}
/// Stable waiting reasons do not rewrite timestamps or manufacture fresh model work.
pub fn wait(
    tx: &mut Tx<'_>,
    permit: &AdminLaunchPermit,
    reason: AdminCaseLaunchReason,
) -> Result<bool> {
    if matches!(
        reason,
        AdminCaseLaunchReason::Claimed
            | AdminCaseLaunchReason::Bound
            | AdminCaseLaunchReason::RestartUnknown
    ) {
        return Err(invalid());
    }
    let Some(mut r) = read(tx.conn(), permit.case_id())? else {
        return Ok(false);
    };
    if r.crew != permit.crew_id()
        || permit.schedule_revision.as_ref() != Some(&r.view.revision)
        || r.view.phase != AdminCaseLaunchPhase::Waiting
        || !current(tx.conn(), &r, tx.now_ms())?
        || admin_case_execution::binding(tx.conn(), &r.crew, &r.case)?.is_some()
    {
        return Ok(false);
    }
    if r.view.reason != reason {
        r.view.reason = reason;
        r.view.updated_at = tx.now_ms();
        save(tx, &mut r)?;
    }
    Ok(true)
}
/// Invoked only inside register_and_bind's transaction. Failure rolls back both.
pub(crate) fn claim(tx: &mut Tx<'_>, permit: &AdminLaunchPermit) -> Result<()> {
    let Some(expected) = &permit.schedule_revision else {
        return Ok(());
    };
    let mut r = read(tx.conn(), permit.case_id())?.ok_or_else(invalid)?;
    if &r.view.revision != expected
        || r.crew != permit.crew_id()
        || r.view.phase != AdminCaseLaunchPhase::Waiting
        || !current(tx.conn(), &r, tx.now_ms())?
    {
        return Err(invalid());
    }
    r.view.phase = AdminCaseLaunchPhase::Dispatching;
    r.view.reason = AdminCaseLaunchReason::Claimed;
    r.view.updated_at = tx.now_ms();
    save(tx, &mut r)
}
pub(crate) fn bound(
    tx: &mut Tx<'_>,
    permit: &AdminLaunchPermit,
    w: &ExecutionWitness,
) -> Result<()> {
    let Some(mut r) = read(tx.conn(), permit.case_id())? else {
        return if permit.schedule_revision.is_none() {
            Ok(())
        } else {
            Err(invalid())
        };
    };
    if !matches!(
        r.view.phase,
        AdminCaseLaunchPhase::Dispatching | AdminCaseLaunchPhase::Waiting
    ) || w.case_id() != r.case
        || w.crew_id() != r.crew
    {
        return Err(invalid());
    }
    r.view.phase = AdminCaseLaunchPhase::Bound;
    r.view.reason = AdminCaseLaunchReason::Bound;
    r.view.session_id = Some(w.session_id().into());
    r.view.attempt_id = Some(w.attempt_id().into());
    r.view.updated_at = tx.now_ms();
    save(tx, &mut r)
}
/// Pure projection. A binding is never inferred from a scheduling phase.
pub fn status(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<AdminCaseLaunchView>> {
    let Some(mut r) = read(c, id)? else {
        return Ok(None);
    };
    if r.crew != crew {
        return Err(invalid());
    }
    if let Some(w) = admin_case_execution::binding(c, crew, id)? {
        if r.view.session_id.as_deref() != Some(w.session_id())
            || r.view.attempt_id.as_deref() != Some(w.attempt_id())
        {
            return Err(invalid());
        }
        r.view.phase = AdminCaseLaunchPhase::Bound;
        r.view.reason = AdminCaseLaunchReason::Bound;
    } else if r.view.phase == AdminCaseLaunchPhase::Bound {
        return Err(invalid());
    } else if r.view.phase == AdminCaseLaunchPhase::Waiting && !current(c, &r, now)? {
        r.view.phase = AdminCaseLaunchPhase::Stale;
        r.view.reason = AdminCaseLaunchReason::StateChanged;
    }
    Ok(Some(r.view))
}
/// Startup only. An orphan committed claim is ambiguous and must never replay.
pub fn recover_page(tx: &mut Tx<'_>, after: Option<String>) -> Result<(usize, Option<String>)> {
    let (refs, next) = admin_cases::source_page(tx.conn(), after)?;
    let mut count = 0;
    for item in refs {
        // Bad optional schedule evidence stays intact; healthy later cases still recover.
        let Ok(Some(mut r)) = read(tx.conn(), &item.id) else {
            continue;
        };
        if r.view.phase == AdminCaseLaunchPhase::Dispatching
            && matches!(
                admin_case_execution::binding(tx.conn(), &r.crew, &r.case),
                Ok(None)
            )
        {
            r.view.phase = AdminCaseLaunchPhase::Unknown;
            r.view.reason = AdminCaseLaunchReason::RestartUnknown;
            r.view.updated_at = tx.now_ms();
            save(tx, &mut r)?;
            count += 1;
        }
    }
    Ok((count, next))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok, store};
    #[test]
    fn adopted_case_has_one_durable_waiting_intent() {
        let mut s = store();
        let case = admin_case_execution::tests::adopted(&mut s);
        assert!(status(s.conn(), "crew-a", &case.id, NOW).unwrap().is_none());
        let p = ok(&mut s, |tx| prepare(tx, "crew-a", &case.id))
            .expect("current native adoption must produce scheduling intent");
        assert_eq!(p.case_id(), case.id);
        assert_eq!(
            status(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .unwrap()
                .phase,
            AdminCaseLaunchPhase::Waiting
        );
    }
    #[test]
    fn stable_wait_is_read_only_and_registration_rolls_back_with_claim() {
        let mut s = store();
        let case = admin_case_execution::tests::adopted(&mut s);
        let p = ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).unwrap();
        assert!(ok(&mut s, |tx| wait(
            tx,
            &p,
            AdminCaseLaunchReason::CapacityUnavailable
        )));
        let before = meta_get(s.conn(), &key(&case.id)).unwrap();
        let changes = s.conn().total_changes();
        assert!(ok(&mut s, |tx| wait(
            tx,
            &p,
            AdminCaseLaunchReason::CapacityUnavailable
        )));
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).is_some());
        assert_eq!(changes, s.conn().total_changes());
        let result: Result<((), Vec<crate::Change>)> = crate::run(&mut s, NOW, |tx| {
            admin_case_execution::register_and_bind(tx, &p)?;
            Err(invalid())
        });
        assert!(result.is_err());
        assert_eq!(before, meta_get(s.conn(), &key(&case.id)).unwrap());
        assert!(
            admin_case_execution::binding(s.conn(), "crew-a", &case.id)
                .unwrap()
                .is_none()
        );
        let r = ok(&mut s, |tx| admin_case_execution::register_and_bind(tx, &p));
        let view = status(s.conn(), "crew-a", &case.id, NOW).unwrap().unwrap();
        assert_eq!(view.phase, AdminCaseLaunchPhase::Bound);
        assert_eq!(view.session_id.as_deref(), Some(r.session.id.as_str()));
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).is_none());
    }
    #[test]
    fn changed_facts_invalidate_permit_and_corrupt_optional_state_is_not_absence() {
        let mut s = store();
        let case = admin_case_execution::tests::adopted(&mut s);
        let p = ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).unwrap();
        s.conn()
            .execute("UPDATE sessions SET desired='stopped' WHERE id='w-a1'", [])
            .unwrap();
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).is_none());
        assert_eq!(
            status(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .unwrap()
                .phase,
            AdminCaseLaunchPhase::Stale
        );
        assert!(
            crate::run(&mut s, NOW, |tx| admin_case_execution::register_and_bind(
                tx, &p
            ))
            .is_err()
        );
        let raw = meta_get(s.conn(), &key(&case.id)).unwrap().unwrap();
        let bad = raw
            .replace("capacity_unavailable", "ready")
            .replace("waiting", "bound");
        meta_set(s.conn(), &key(&case.id), &bad).unwrap();
        assert!(status(s.conn(), "crew-a", &case.id, NOW).is_err());
        assert_eq!(meta_get(s.conn(), &key(&case.id)).unwrap().unwrap(), bad);
    }
    #[test]
    fn orphan_claim_recovery_is_unknown_and_never_replays() {
        let mut s = store();
        let case = admin_case_execution::tests::adopted(&mut s);
        let p = ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).unwrap();
        // Explicit abnormal persisted-boundary fixture; normal registration is atomic.
        ok(&mut s, |tx| claim(tx, &p));
        assert_eq!(ok(&mut s, |tx| recover_page(tx, None)).0, 1);
        assert_eq!(
            status(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .unwrap()
                .phase,
            AdminCaseLaunchPhase::Unknown
        );
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).is_none());
    }

    #[test]
    fn real_file_reopen_keeps_waiting_and_bound_without_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launch.db");
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

        let mut s = file;
        let case = admin_case_execution::tests::adopted(&mut s);
        let p = ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).unwrap();
        ok(&mut s, |tx| {
            wait(tx, &p, AdminCaseLaunchReason::CapacityUnavailable)
        });
        let before = status(s.conn(), "crew-a", &case.id, NOW).unwrap();
        s.finish().unwrap();
        drop(s);
        let mut s = crewd_store::Store::open(&path).unwrap();
        assert_eq!(before, status(s.conn(), "crew-a", &case.id, NOW).unwrap());
        let p = ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).unwrap();
        let reg = ok(&mut s, |tx| admin_case_execution::register_and_bind(tx, &p));
        let before = status(s.conn(), "crew-a", &case.id, NOW).unwrap();
        s.finish().unwrap();
        drop(s);
        let mut s = crewd_store::Store::open(&path).unwrap();
        assert_eq!(before, status(s.conn(), "crew-a", &case.id, NOW).unwrap());
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &case.id)).is_none());
        assert_eq!(
            admin_case_execution::binding(s.conn(), "crew-a", &case.id)
                .unwrap()
                .unwrap()
                .session_id(),
            reg.session.id
        );
        s.finish().unwrap();
    }
}
