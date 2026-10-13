//! Read-only global idle candidates and atomic acceptance. No runner calls.
use crate::{Result, Sender, Tx, asks, crews, messages, wakes, work};
use crewd_store::{StoreError, object_refs::ObjectKind, work_stop_checks as budget};
use crewd_work::{
    Config, SessionId, StopDecision, StopKind, StopStreak, StopViolation, Ts, advance_stop_streak,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    id: String,
    revision: String,
    kind: StopKind,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    session: String,
    expected_epoch: Option<i64>,
    targets: Vec<Target>,
}
fn invalid(s: &str) -> crate::CollabError {
    StoreError::Invalid(s.into()).into()
}
fn targets(conn: &Connection, session: &str, now: i64) -> Result<Vec<Target>> {
    // Include missing authority, rather than silently treating it as clean.
    let crews = {
        let mut q=conn.prepare("SELECT DISTINCT c.id FROM crews c LEFT JOIN work_books b ON b.crew_id=c.id WHERE c.archived_at IS NULL AND (EXISTS(SELECT 1 FROM json_each(b.state,'$.items') i WHERE json_extract(i.value,'$.owner')=?1) OR EXISTS(SELECT 1 FROM object_refs r WHERE r.crew_id=c.id AND r.kind='plan' AND (b.state IS NULL OR json_type(b.state,'$.items.'||substr(r.source_id,6)) IS NULL))) ORDER BY c.id")?;
        q.query_map([session], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut out = Vec::new();
    for crew in crews {
        let b = asks::load(conn, &crew)?;
        for v in b.stop_violations(&SessionId::new(session), Ts(now)) {
            out.push(Target {
                id: work::reference(conn, ObjectKind::Plan, &crew, v.item)?,
                revision: b
                    .item(v.item)
                    .expect("violation has item")
                    .revision
                    .to_string(),
                kind: v.kind,
            });
        }
    }
    out.sort_by(|a, b| {
        (work::number(&a.id).expect("issued ID"), a.kind)
            .cmp(&(work::number(&b.id).expect("issued ID"), b.kind))
    });
    Ok(out)
}
fn decision(streak: &mut StopStreak, targets: &[Target]) -> Result<StopDecision> {
    let v = targets
        .iter()
        .map(|t| {
            Ok(StopViolation {
                item: t
                    .id
                    .parse()
                    .map_err(|_| invalid("invalid stop-check identity"))?,
                kind: t.kind,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(advance_stop_streak(streak, v, &Config::default()))
}
fn eligible(conn: &Connection, session: &str) -> Result<bool> {
    let Some(s) = crewd_store::sessions::get(conn, session)? else {
        return Ok(false);
    };
    Ok(s.desired == crewd_proto::Desired::Running && !crews::is_archived(conn, &s.crew_id)?)
}
fn queued(conn: &Connection, session: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM wakes WHERE session_id=?1 AND kind='stop_check' AND source LIKE 'work-stop:%' AND state IN ('pending','inflight'))",[session],|r|r.get(0))?)
}
/// Reading does not change the budget, create a book, or enqueue a wake.
pub fn candidate(conn: &Connection, session: &str, now: i64) -> Result<Option<Candidate>> {
    if !eligible(conn, session)? {
        return Ok(None);
    }
    let targets = targets(conn, session, now)?;
    if !targets.is_empty() && queued(conn, session)? {
        return Ok(None);
    }
    let prior = budget::get(conn, session)?;
    let mut streak = prior.as_ref().map(|b| b.streak.clone()).unwrap_or_default();
    let d = decision(&mut streak, &targets)?;
    if matches!(d, StopDecision::Suppressed(_))
        || (matches!(d, StopDecision::Clean)
            && prior
                .as_ref()
                .is_none_or(|p| p.streak == StopStreak::default()))
    {
        return Ok(None);
    }
    Ok(Some(Candidate {
        session: session.into(),
        expected_epoch: prior.map(|b| b.epoch),
        targets,
    }))
}
#[derive(Debug, PartialEq, Eq)]
pub enum Accepted {
    Stale,
    Clean,
    Rewake(i64),
    Escalated,
    Suppressed,
}
/// The caller must first accept the current actor generation. All persistent
/// facts are checked again in this transaction; rejected candidates cost no budget.
pub fn accept(tx: &mut Tx<'_>, c: &Candidate) -> Result<Accepted> {
    if !eligible(tx.conn(), &c.session)? {
        return Ok(Accepted::Stale);
    }
    let prior = budget::get(tx.conn(), &c.session)?;
    if prior.as_ref().map(|b| b.epoch) != c.expected_epoch
        || targets(tx.conn(), &c.session, tx.now_ms())? != c.targets
    {
        return Ok(Accepted::Stale);
    }
    if !c.targets.is_empty() && queued(tx.conn(), &c.session)? {
        return Ok(Accepted::Stale);
    }
    let mut streak = prior.map(|b| b.streak).unwrap_or_default();
    let d = decision(&mut streak, &c.targets)?;
    if matches!(d, StopDecision::Suppressed(_)) {
        return Ok(Accepted::Suppressed);
    }
    if !budget::save(tx.conn(), &c.session, c.expected_epoch, &streak)? {
        return Ok(Accepted::Stale);
    }
    let note = serde_json::json!({"stopWork":c.targets.iter().take(50).map(|t|serde_json::json!({"id":t.id,"kind":t.kind})).collect::<Vec<_>>(),"total":c.targets.len(),"partial":c.targets.len()>50});
    match d {
        StopDecision::Clean => Ok(Accepted::Clean),
        StopDecision::Rewake(_) => {
            let row = wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: c.session.clone(),
                    kind: wakes::WakeKind::StopCheck,
                    source: format!("work-stop:{}", uuid::Uuid::new_v4()),
                    priority: wakes::WakePriority::System,
                    not_before_ms: None,
                    note: Some(
                        serde_json::json!({"summary":note,"stopTargets":c.targets}).to_string(),
                    ),
                    origin: None,
                },
            )?;
            match row {
                wakes::Enqueued::New(id) => Ok(Accepted::Rewake(id)),
                _ => Err(invalid("accepted stop-check did not enqueue")),
            }
        }
        StopDecision::Escalate(_) => {
            let home = crews::management(tx.conn())?
                .ok_or_else(|| invalid("stop escalation requires management crew"))?;
            messages::announce_to_human(
                tx,
                &home,
                Sender::System,
                format!("Stop-check escalation for {}: {}", c.session, note),
                "work_stop_escalation",
            )?;
            Ok(Accepted::Escalated)
        }
        StopDecision::Suppressed(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        testkit::{NOW, ok, store},
        work,
    };
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    fn seed(s: &mut crewd_store::Store, crew: &str) -> crewd_proto::WorkPlan {
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='w-a1'", [])
            .unwrap();
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    owner: Some("w-a1".into()),
                    title: "synthetic stop work".into(),
                    ..Default::default()
                },
            )
        })
    }
    fn next(s: &mut crewd_store::Store) -> Accepted {
        s.conn()
            .execute(
                "UPDATE wakes SET state='delivered' WHERE session_id='w-a1'",
                [],
            )
            .unwrap();
        let c = candidate(s.conn(), "w-a1", NOW).unwrap().unwrap();
        ok(s, |tx| accept(tx, &c))
    }
    #[test]
    fn global_idle_read_is_free_and_three_reminders_escalate_once() {
        let mut s = store();
        seed(&mut s, "crew-a");
        seed(&mut s, "crew-b");
        let c = candidate(s.conn(), "w-a1", NOW).unwrap().unwrap();
        assert_eq!(c.targets.len(), 2);
        assert_ne!(c.targets[0].id, c.targets[1].id);
        for _ in 0..5 {
            assert!(candidate(s.conn(), "w-a1", NOW).unwrap().is_some());
        }
        assert!(budget::get(s.conn(), "w-a1").unwrap().is_none());
        assert!(matches!(
            ok(&mut s, |tx| accept(tx, &c)),
            Accepted::Rewake(_)
        ));
        assert_eq!(ok(&mut s, |tx| accept(tx, &c)), Accepted::Stale);
        assert!(candidate(s.conn(), "w-a1", NOW).unwrap().is_none());
        assert!(matches!(next(&mut s), Accepted::Rewake(_)));
        assert!(matches!(next(&mut s), Accepted::Rewake(_)));
        assert_eq!(next(&mut s), Accepted::Escalated);
        assert!(candidate(s.conn(), "w-a1", NOW).unwrap().is_none());
        assert_eq!(
            budget::get(s.conn(), "w-a1")
                .unwrap()
                .unwrap()
                .streak
                .rewakes,
            3
        );
        assert_eq!(wakes::list(s.conn(), Some("w-a1"), 100).unwrap().len(), 3);
        let n: i64 = s
            .conn()
            .query_row(
                "SELECT count(*) FROM messages WHERE category='work_stop_escalation'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
    #[test]
    fn changed_revision_or_human_stop_rejects_without_budget() {
        let mut s = store();
        let p = seed(&mut s, "crew-a");
        let c = candidate(s.conn(), "w-a1", NOW).unwrap().unwrap();
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: p.id,
                    expected_revision: p.revision,
                    status: Some(WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 60_000)),
                    note: Some("real next step".into()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(ok(&mut s, |tx| accept(tx, &c)), Accepted::Stale);
        assert!(budget::get(s.conn(), "w-a1").unwrap().is_none());
        seed(&mut s, "crew-b");
        let c = candidate(s.conn(), "w-a1", NOW).unwrap().unwrap();
        s.conn()
            .execute("UPDATE sessions SET desired='stopped' WHERE id='w-a1'", [])
            .unwrap();
        assert_eq!(ok(&mut s, |tx| accept(tx, &c)), Accepted::Stale);
        assert!(budget::get(s.conn(), "w-a1").unwrap().is_none());
    }
    #[test]
    fn queue_failure_rolls_back_budget_and_missing_book_is_not_clean() {
        let mut s = store();
        seed(&mut s, "crew-a");
        let c = candidate(s.conn(), "w-a1", NOW).unwrap().unwrap();
        s.conn().execute_batch("CREATE TRIGGER deny_stop BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'synthetic queue failure'); END;").unwrap();
        assert!(crate::run(&mut s, NOW, |tx| accept(tx, &c)).is_err());
        assert!(budget::get(s.conn(), "w-a1").unwrap().is_none());
        s.conn()
            .execute("DELETE FROM work_books WHERE crew_id='crew-a'", [])
            .unwrap();
        assert!(candidate(s.conn(), "w-a1", NOW).is_err());
    }
    #[test]
    fn clean_resets_without_reusing_epoch_and_escalation_failure_retries() {
        let mut s = store();
        let p = seed(&mut s, "crew-a");
        assert!(matches!(next(&mut s), Accepted::Rewake(_)));
        let old = candidate_after_settle(&mut s);
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: p.id,
                    expected_revision: p.revision,
                    status: Some(WorkUpdateStatus::Done),
                    ..Default::default()
                },
            )
        });
        assert_eq!(ok(&mut s, |tx| accept(tx, &old)), Accepted::Stale);
        assert_eq!(next(&mut s), Accepted::Clean);
        let b = budget::get(s.conn(), "w-a1").unwrap().unwrap();
        assert_eq!(b.epoch, 2);
        assert_eq!(b.streak, StopStreak::default());
        assert!(candidate(s.conn(), "w-a1", NOW).unwrap().is_none());
        seed(&mut s, "crew-b");
        for _ in 0..3 {
            assert!(matches!(next(&mut s), Accepted::Rewake(_)));
        }
        let c = candidate_after_settle(&mut s);
        let before = budget::get(s.conn(), "w-a1").unwrap();
        s.conn().execute_batch("CREATE TRIGGER deny_escalation BEFORE INSERT ON messages WHEN NEW.category='work_stop_escalation' BEGIN SELECT RAISE(ABORT,'synthetic announcement failure'); END;").unwrap();
        assert!(crate::run(&mut s, NOW, |tx| accept(tx, &c)).is_err());
        assert_eq!(budget::get(s.conn(), "w-a1").unwrap(), before);
        s.conn()
            .execute_batch("DROP TRIGGER deny_escalation;")
            .unwrap();
        assert_eq!(ok(&mut s, |tx| accept(tx, &c)), Accepted::Escalated);
    }
    fn candidate_after_settle(s: &mut crewd_store::Store) -> Candidate {
        s.conn()
            .execute(
                "UPDATE wakes SET state='delivered' WHERE session_id='w-a1'",
                [],
            )
            .unwrap();
        candidate(s.conn(), "w-a1", NOW).unwrap().unwrap()
    }
}
