//! Deterministic facts only: no model decision, reassignment or second work book.
use crate::{Result, Tx, asks, channels, crews, wakes, work};
use crewd_store::{StoreError, object_refs::ObjectKind, work_followups as durable};
use crewd_work::{Blocker, LostCause, Ts, WaitTier, WorkId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
fn invalid(s: &str) -> crate::CollabError {
    StoreError::Invalid(s.into()).into()
}
fn hash(s: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, s.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Cause {
    Checkpoint {
        owner: String,
        deadline: i64,
    },
    OwnerLost {
        owner: String,
        failure: Option<String>,
        missed: Option<u32>,
        checkpoint: Option<i64>,
    },
    HumanWait {
        ask: String,
        asked_at: i64,
        tier: u8,
    },
    DependencyLost {
        dependency: String,
        owner: String,
        failure: Option<String>,
        missed: Option<u32>,
        checkpoint: Option<i64>,
    },
}
impl Cause {
    fn category(&self) -> &'static str {
        match self {
            Self::Checkpoint { .. } => "checkpoint",
            Self::OwnerLost { .. } => "owner_lost",
            Self::HumanWait { .. } => "human_wait",
            Self::DependencyLost { .. } => "dependency_lost",
        }
    }
}
#[derive(Clone)]
struct Fact {
    id: String,
    subject: String,
    number: i64,
    crew: String,
    recipient: String,
    cause: Cause,
    allowed: bool,
}
struct Loss {
    owner: String,
    failure: Option<String>,
    missed: Option<u32>,
    checkpoint: Option<i64>,
}
struct Attribution {
    effect: String,
    stop_floor: i64,
    wake_id: Option<i64>,
    original_wake_id: Option<i64>,
    recipient: String,
}
fn loss(
    conn: &Connection,
    book: &crewd_work::WorkBook,
    id: WorkId,
    now: i64,
) -> Result<Option<Loss>> {
    let item = book
        .item(id)
        .ok_or_else(|| invalid("followup dependency lacks authority"))?;
    let Some(owner) = &item.owner else {
        return Ok(None);
    };
    let Some(session) = crewd_store::sessions::get(conn, owner.as_str())? else {
        return Err(invalid("followup owner lacks session authority"));
    };
    let patrol = book
        .patrol_item(id, Ts(now), session.resume_failed_at.is_some())
        .ok_or_else(|| invalid("followup patrol lacks authority"))?;
    Ok(match patrol.lost {
        Some(LostCause::Unrecoverable) => Some(Loss {
            owner: owner.to_string(),
            failure: session.resume_failed_at,
            missed: None,
            checkpoint: None,
        }),
        Some(LostCause::Silent { missed })
            if !wakes::automatic_work_paused(conn, owner.as_str())? =>
        {
            Some(Loss {
                owner: owner.to_string(),
                failure: None,
                missed: Some(missed),
                checkpoint: item.next_check_at.map(|t| t.0),
            })
        }
        _ => None,
    })
}
fn collect(conn: &Connection, now: i64, baseline: bool) -> Result<Vec<Fact>> {
    let book = asks::load_org(conn)?;
    let observed_now = now.max(book.state().clock.0);
    let due: BTreeSet<WorkId> = book
        .due_checkpoints(Ts(now))
        .into_iter()
        .flat_map(|d| d.items)
        .collect();
    let mut out = Vec::new();
    for (&local, item) in &book.state().items {
        if item.status.is_terminal() {
            continue;
        }
        let crew = book
            .state()
            .item_crew(local)
            .ok_or_else(|| invalid("followup item lacks crew attribution"))?
            .to_string();
        let subject = work::reference(conn, ObjectKind::Plan, &crew, local)?;
        let number = work::number(&subject)?;
        let captain = crews::captain_of(conn, &crew)?.unwrap_or_default();
        let mut causes = Vec::new();
        if let (Some(owner), Some(deadline)) = (&item.owner, item.next_check_at)
            && (baseline || due.contains(&local))
        {
            causes.push((
                owner.to_string(),
                Cause::Checkpoint {
                    owner: owner.to_string(),
                    deadline: deadline.0,
                },
            ));
        }
        if let Some(Loss {
            owner,
            failure,
            missed,
            checkpoint,
        }) = loss(conn, &book, local, now)?
        {
            causes.push((
                captain.clone(),
                Cause::OwnerLost {
                    owner,
                    failure,
                    missed,
                    checkpoint,
                },
            ));
        }
        for blocker in &item.blocked_on {
            match blocker {
                Blocker::Human(id) => {
                    let ask = book
                        .state()
                        .asks
                        .get(id)
                        .ok_or_else(|| invalid("followup blocker lacks ask authority"))?;
                    if ask.state != crewd_work::AskState::Open {
                        continue;
                    }
                    let tier = crewd_work::wait_tier(
                        observed_now.saturating_sub(ask.asked_at.0).max(0),
                        book.config(),
                    );
                    let tier = match tier {
                        WaitTier::Fresh => continue,
                        WaitTier::Pinned => 1,
                        WaitTier::StillWanted => 2,
                    };
                    let ask_crew = book
                        .state()
                        .ask_crew(*id)
                        .ok_or_else(|| invalid("followup ask lacks crew attribution"))?;
                    // Human ask content never crosses its source crew or private scope.
                    if ask_crew.as_str() != crew {
                        continue;
                    }
                    let issued = work::reference(conn, ObjectKind::HumanTask, &crew, id)?;
                    causes.push((
                        captain.clone(),
                        Cause::HumanWait {
                            ask: issued,
                            asked_at: ask.asked_at.0,
                            tier,
                        },
                    ));
                }
                Blocker::Item(dependency) => {
                    if let Some(Loss {
                        owner,
                        failure,
                        missed,
                        checkpoint,
                    }) = loss(conn, &book, *dependency, now)?
                    {
                        let dependency_crew = book
                            .state()
                            .item_crew(*dependency)
                            .ok_or_else(|| invalid("followup dependency lacks crew attribution"))?;
                        if dependency_crew.as_str() != crew {
                            continue;
                        }
                        let dependency =
                            work::reference(conn, ObjectKind::Plan, &crew, dependency)?;
                        causes.push((
                            captain.clone(),
                            Cause::DependencyLost {
                                dependency,
                                owner,
                                failure,
                                missed,
                                checkpoint,
                            },
                        ));
                    }
                }
            }
        }
        for (recipient, cause) in causes {
            let target = crewd_store::sessions::get(conn, &recipient)?;
            let role_ok = match &cause {
                Cause::Checkpoint { owner, .. } => owner == &recipient,
                _ => {
                    target
                        .as_ref()
                        .is_some_and(|t| t.crew_id == crew && t.role == crewd_proto::Role::Captain)
                        && captain == recipient
                }
            };
            let scope_ok = match &item.channel {
                Some(c) => channels::readable_by(conn, c.as_str(), &recipient)?,
                None => true,
            };
            let allowed = !recipient.is_empty()
                && role_ok
                && scope_ok
                && !crews::is_archived(conn, &crew)?
                && target
                    .as_ref()
                    .is_some_and(|t| t.resume_failed_at.is_none())
                && !wakes::automatic_work_paused(conn, &recipient)?
                && match &target {
                    Some(t) => !crews::is_archived(conn, &t.crew_id)?,
                    None => false,
                };
            let identity = serde_json::to_string(&(&subject, &recipient, &cause))
                .map_err(|_| invalid("followup semantic identity serialization"))?;
            out.push(Fact {
                id: hash(&identity),
                subject: subject.clone(),
                number,
                crew: crew.clone(),
                recipient,
                cause,
                allowed,
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}
fn observe(tx: &mut Tx<'_>, f: &Fact, suppressed: bool) -> Result<bool> {
    let exists: bool = tx.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM work_followup_facts WHERE id=?1)",
        [&f.id],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(false);
    }
    let floor = if f.recipient.is_empty() {
        0
    } else {
        wakes::human_stop_floor(tx.conn(), &f.recipient)?
    };
    let recipient = crewd_store::sessions::get(tx.conn(), &f.recipient)?.map(|s| s.id);
    let cause =
        serde_json::to_string(&f.cause).map_err(|_| invalid("followup cause serialization"))?;
    tx.conn().execute("INSERT INTO work_followup_facts(id,subject_number,crew_id,category,cause,recipient_snapshot,recipient_id,stop_floor,effect,observed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![f.id,f.number,f.crew,f.cause.category(),cause,f.recipient,recipient,floor,if suppressed{"suppressed"}else{"observed"},tx.now_ts()])?;
    Ok(true)
}
/// Called before either checkpoint or captain collection inside the same Tx.
pub fn baseline(tx: &mut Tx<'_>) -> Result<()> {
    if durable::available(tx.conn())? && durable::baseline_required(tx.conn())? {
        for f in collect(tx.conn(), tx.now_ms(), true)? {
            observe(tx, &f, true)?;
        }
        durable::finish_baseline(tx.conn())?;
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    facts: Vec<String>,
}
fn source(ids: &[String]) -> String {
    format!("work-followup:{}", hash(&ids.join("\n")))
}
fn link(tx: &mut Tx<'_>, ids: &[String], wake: i64) -> Result<()> {
    for id in ids {
        if tx.conn().execute("UPDATE work_followup_facts SET wake_id=?2,original_wake_id=?2,effect='enqueued' WHERE id=?1 AND effect='observed' AND original_wake_id IS NULL",params![id,wake])?!=1 { return Err(invalid("followup enqueue lost durable attribution")); }
    }
    Ok(())
}
/// At most50 genuinely new semantic facts per recipient in each merged wake.
pub fn enqueue(tx: &mut Tx<'_>) -> Result<usize> {
    if !durable::available(tx.conn())? {
        return Ok(0);
    }
    baseline(tx)?;
    let mut targets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in collect(tx.conn(), tx.now_ms(), false)? {
        if matches!(f.cause, Cause::Checkpoint { .. }) {
            if !f.allowed {
                observe(tx, &f, true)?;
            }
            continue;
        }
        if !f.allowed {
            observe(tx, &f, true)?;
            continue;
        }
        let group = targets.entry(f.recipient.clone()).or_default();
        if group.len() < 50 && observe(tx, &f, false)? {
            group.push(f.id)
        }
    }
    let mut count = 0;
    for (recipient, ids) in targets {
        if ids.is_empty() {
            continue;
        }
        let note = serde_json::to_string(&Manifest { facts: ids.clone() })
            .map_err(|_| invalid("followup manifest serialization"))?;
        match wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: recipient,
                kind: wakes::WakeKind::System,
                source: source(&ids),
                priority: wakes::WakePriority::System,
                not_before_ms: None,
                note: Some(note),
                origin: None,
            },
        )? {
            wakes::Enqueued::New(id) | wakes::Enqueued::Duplicate(id) => {
                link(tx, &ids, id)?;
                count += 1
            }
            wakes::Enqueued::Skipped => {
                for id in ids {
                    tx.conn().execute(
                        "UPDATE work_followup_facts SET effect='suppressed' WHERE id=?1",
                        [id],
                    )?;
                }
            }
        }
    }
    Ok(count)
}
/// Revision/note changes are irrelevant to semantic checkpoint identity.
pub(crate) fn checkpoint_new(
    tx: &mut Tx<'_>,
    subject: &str,
    owner: &str,
    deadline: i64,
) -> Result<bool> {
    if !durable::available(tx.conn())? {
        return Ok(true);
    }
    baseline(tx)?;
    let found=collect(tx.conn(),tx.now_ms(),false)?.into_iter().find(|f|f.subject==subject && matches!(&f.cause,Cause::Checkpoint{owner:o,deadline:d} if o==owner && *d==deadline));
    let Some(f) = found else {
        return Err(invalid("checkpoint fact lacks due authority"));
    };
    Ok(observe(tx, &f, !f.allowed)? && f.allowed)
}
pub(crate) fn checkpoint_link(
    tx: &mut Tx<'_>,
    subjects: &[(String, i64)],
    owner: &str,
    wake: i64,
) -> Result<()> {
    if !durable::available(tx.conn())? {
        return Ok(());
    }
    let facts = collect(tx.conn(), tx.now_ms(), false)?;
    let ids: Vec<String> = facts
        .into_iter()
        .filter(|f| {
            subjects.iter().any(|(s, d)| {
                s == &f.subject
                    && matches!(&f.cause,Cause::Checkpoint{owner:o,deadline:x} if o==owner && x==d)
            })
        })
        .map(|f| f.id)
        .collect();
    if ids.len() != subjects.len() {
        return Err(invalid("checkpoint link lacks complete authority"));
    };
    link(tx, &ids, wake)
}
fn current_ids(
    conn: &Connection,
    row: &wakes::WakeRow,
    now: i64,
    acknowledged: bool,
) -> Result<BTreeSet<String>> {
    let actual_delivered: bool = acknowledged
        && conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM wakes WHERE id=?1 AND state='delivered')",
            [row.id],
            |r| r.get(0),
        )?;
    let mut current = BTreeSet::new();
    for f in collect(conn, now, false)? {
        if !f.allowed || f.recipient != row.session {
            continue;
        }
        let saved:Option<Attribution>=conn.query_row("SELECT effect,stop_floor,wake_id,original_wake_id,recipient_snapshot FROM work_followup_facts WHERE id=?1",[&f.id],|r|Ok(Attribution{effect:r.get(0)?,stop_floor:r.get(1)?,wake_id:r.get(2)?,original_wake_id:r.get(3)?,recipient:r.get(4)?})).optional()?;
        if let Some(Attribution {
            effect,
            stop_floor: floor,
            wake_id: linked,
            original_wake_id: original,
            recipient,
        }) = saved
            && (effect == "enqueued" || (effect == "delivered" && actual_delivered))
            && linked == Some(row.id)
            && original == Some(row.id)
            && recipient == row.session
            && floor == wakes::human_stop_floor(conn, &row.session)?
            && row.id > floor
        {
            current.insert(f.id);
        }
    }
    Ok(current)
}
pub(crate) fn checkpoint_admitted(
    conn: &Connection,
    row: &wakes::WakeRow,
    now: i64,
) -> Result<bool> {
    if !durable::available(conn)? {
        return Ok(true);
    }
    let attributed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM work_followup_facts WHERE original_wake_id=?1)",
        [row.id],
        |r| r.get(0),
    )?;
    Ok(!attributed || !current_ids(conn, row, now, false)?.is_empty())
}
pub(crate) fn render(
    conn: &Connection,
    row: &wakes::WakeRow,
    now: i64,
) -> Result<work::FollowupNote> {
    if row.kind != wakes::WakeKind::System || row.priority != wakes::WakePriority::System {
        return Err(invalid("followup wake class invalid"));
    }
    let m: Manifest = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| invalid("followup manifest invalid"))?;
    if m.facts.is_empty()
        || m.facts.len() > 50
        || m.facts.windows(2).any(|w| w[0] >= w[1])
        || row.source != source(&m.facts)
    {
        return Err(invalid("followup source identity invalid"));
    }
    let current = current_ids(conn, row, now, false)?;
    let facts = collect(conn, now, false)?;
    let mut reasons = Vec::new();
    for id in m.facts {
        let linked:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM work_followup_facts WHERE id=?1 AND original_wake_id=?2 AND recipient_snapshot=?3)",params![id,row.id,row.session],|r|r.get(0))?;
        if !linked {
            return Err(invalid("followup manifest lacks durable wake attribution"));
        }
        if current.contains(&id) {
            let f = facts
                .iter()
                .find(|f| f.id == id)
                .ok_or_else(|| invalid("followup current fact missing"))?;
            reasons.push(serde_json::json!({"work":f.subject,"reason":f.cause.category()}));
        }
    }
    if reasons.is_empty() {
        Ok(work::FollowupNote::Stale)
    } else {
        Ok(work::FollowupNote::Current(serde_json::json!({"deterministicFollowup":reasons,"total":reasons.len(),"partial":false}).to_string()))
    }
}

pub(crate) fn checkpoint_source(subjects: &[(String, i64)], owner: &str) -> String {
    let mut ids: Vec<String> = subjects
        .iter()
        .map(|(s, d)| {
            hash(
                &serde_json::to_string(&(
                    s,
                    owner,
                    Cause::Checkpoint {
                        owner: owner.into(),
                        deadline: *d,
                    },
                ))
                .expect("finite fact metadata"),
            )
        })
        .collect();
    ids.sort();
    format!("work-check:{}", hash(&ids.join("\n")))
}
pub(crate) fn checkpoint_subject_admitted(
    conn: &Connection,
    row: &wakes::WakeRow,
    subject: &str,
    deadline: i64,
    now: i64,
    acknowledged: bool,
) -> Result<bool> {
    if !durable::available(conn)? {
        return Ok(true);
    }
    let attributed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM work_followup_facts WHERE original_wake_id=?1)",
        [row.id],
        |r| r.get(0),
    )?;
    if !attributed {
        return Ok(true);
    }
    let id = hash(
        &serde_json::to_string(&(
            subject,
            &row.session,
            Cause::Checkpoint {
                owner: row.session.clone(),
                deadline,
            },
        ))
        .map_err(|_| invalid("checkpoint identity serialization"))?,
    );
    Ok(current_ids(conn, row, now, acknowledged)?.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{MIN, NOW, ok, ok_at, store},
    };
    use crewd_proto::WorkCreateParams;
    fn plan(s: &mut crewd_store::Store) -> String {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "private body must not be copied".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        })
        .id
    }
    fn failure(s: &crewd_store::Store, value: &str) {
        s.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at=?1 WHERE id='w-a1'",
                [value],
            )
            .unwrap();
    }
    fn captain_rows(s: &crewd_store::Store) -> Vec<wakes::WakeRow> {
        wakes::list(s.conn(), Some("cap-a"), 100)
            .unwrap()
            .into_iter()
            .filter(|w| w.source.starts_with("work-followup:"))
            .collect()
    }
    fn projection(s: &crewd_store::Store) -> Vec<(String, String, Option<i64>, Option<i64>)> {
        let mut q = s
            .conn()
            .prepare(
                "SELECT id,effect,wake_id,original_wake_id FROM work_followup_facts ORDER BY id",
            )
            .unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    #[test]
    fn merge_only_new_semantic_facts_and_prune_does_not_repeat_unknown_delivery() {
        let mut s = store();
        let a = plan(&mut s);
        let b = plan(&mut s);
        failure(&s, "actual-failure-1");
        assert_eq!(ok(&mut s, enqueue), 1);
        let first = captain_rows(&s)[0].clone();
        let work::FollowupNote::Current(body) = render(s.conn(), &first, NOW).unwrap() else {
            panic!("eligible")
        };
        assert!(body.contains(&a) && body.contains(&b));
        assert!(!body.contains("private body"));
        assert_eq!(projection(&s).len(), 2);
        assert!(projection(&s).iter().all(|r| r.1 == "enqueued"));
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue).0, 0);
        ok(&mut s, |tx| wakes::claim(tx, &[first.id]));
        ok(&mut s, |tx| {
            wakes::apply(tx, &[first.id], wakes::WakeEvent::Requeue)
        });
        assert!(matches!(
            render(s.conn(), &first, NOW).unwrap(),
            work::FollowupNote::Stale
        ));
        assert!(projection(&s).iter().all(|r| r.1 == "unknown"));
        ok(&mut s, |tx| {
            wakes::mark_dropped(tx, first.id, "actual result unknown")
        });
        assert_eq!(
            ok_at(&mut s, NOW + MIN, |tx| wakes::prune(tx, NOW + MIN, 100)).0,
            1
        );
        assert!(
            projection(&s)
                .iter()
                .all(|r| r.2.is_none() && r.3 == Some(first.id))
        );
        assert_eq!(ok_at(&mut s, NOW + 2 * MIN, enqueue).0, 0);
        let c = plan(&mut s);
        assert_eq!(ok(&mut s, enqueue), 1);
        let next = captain_rows(&s)[0].clone();
        let work::FollowupNote::Current(body) = render(s.conn(), &next, NOW).unwrap() else {
            panic!("new fact")
        };
        assert!(body.contains(&c));
        assert!(!body.contains(&a) && !body.contains(&b));
    }
    #[test]
    fn stopped_observation_does_not_revive_but_new_actual_failure_can_progress() {
        let mut s = store();
        plan(&mut s);
        failure(&s, "actual-failure-1");
        ok(&mut s, |tx| wakes::human_stop(tx, "cap-a"));
        assert_eq!(ok(&mut s, enqueue), 0);
        assert_eq!(projection(&s)[0].1, "suppressed");

        // Resume authority uses the existing human stop latch, never wall-clock expiry.
        ok(&mut s, |tx| {
            crate::sessions::set_desired(tx, "cap-a", crewd_proto::Desired::Running)
        });
        assert_eq!(ok(&mut s, enqueue), 0);
        failure(&s, "actual-failure-2");
        assert_eq!(ok(&mut s, enqueue), 1);
    }
    #[test]
    fn old_snapshot_baseline_observes_history_and_new_cause_is_separate() {
        let mut s = store();
        plan(&mut s);
        failure(&s, "old-failure");
        durable::restored(s.conn()).unwrap();
        assert!(durable::baseline_required(s.conn()).unwrap());
        assert_eq!(ok(&mut s, enqueue), 0);
        assert!(!durable::baseline_required(s.conn()).unwrap());
        assert_eq!(projection(&s)[0].1, "suppressed");
        failure(&s, "new-authoritative-failure");
        assert_eq!(ok(&mut s, enqueue), 1);
        let row = captain_rows(&s)[0].clone();
        durable::restored(s.conn()).unwrap();
        assert!(matches!(
            render(s.conn(), &row, NOW).unwrap(),
            work::FollowupNote::Stale
        ));
        assert_eq!(ok(&mut s, enqueue), 0);
    }
    #[test]
    fn queue_sql_fault_rolls_back_observations_and_stop_floor() {
        let mut s = store();
        plan(&mut s);
        failure(&s, "actual-failure");
        s.conn().execute_batch("CREATE TRIGGER deny_followup BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'actual queue fault'); END;").unwrap();
        assert!(crate::run(&mut s, NOW, enqueue).is_err());
        assert!(projection(&s).is_empty());
        assert!(captain_rows(&s).is_empty());
        s.conn()
            .execute_batch("DROP TRIGGER deny_followup")
            .unwrap();
        assert_eq!(ok(&mut s, enqueue), 1);
    }
    #[test]
    fn bounded_batches_never_include_old_attributed_facts() {
        let mut s = store();
        for _ in 0..51 {
            plan(&mut s);
        }
        failure(&s, "actual-failure");
        assert_eq!(ok(&mut s, enqueue), 1);
        assert_eq!(projection(&s).len(), 50);
        assert_eq!(ok(&mut s, enqueue), 1);
        assert_eq!(projection(&s).len(), 51);
        let rows = captain_rows(&s);
        let a: Manifest = serde_json::from_str(rows[0].note.as_ref().unwrap()).unwrap();
        let b: Manifest = serde_json::from_str(rows[1].note.as_ref().unwrap()).unwrap();
        assert!(
            (a.facts.len() == 50 && b.facts.len() == 1)
                || (a.facts.len() == 1 && b.facts.len() == 50)
        );
        assert!(a.facts.iter().all(|id| !b.facts.contains(id)));
        assert_eq!(ok(&mut s, enqueue), 0);
    }
    #[test]
    fn forged_source_or_missing_attribution_never_renders_and_role_changes_stale() {
        let mut s = store();
        plan(&mut s);
        failure(&s, "actual-failure");
        ok(&mut s, enqueue);
        let mut row = captain_rows(&s)[0].clone();
        let real = row.source.clone();
        row.source.push('x');
        assert!(render(s.conn(), &row, NOW).is_err());
        row.source = real;
        s.conn()
            .execute("UPDATE sessions SET role='worker' WHERE id='cap-a'", [])
            .unwrap();
        assert!(matches!(
            render(s.conn(), &row, NOW).unwrap(),
            work::FollowupNote::Stale
        ));
        s.conn()
            .execute("UPDATE sessions SET role='captain' WHERE id='cap-a'", [])
            .unwrap();
        s.conn()
            .execute(
                "UPDATE work_followup_facts SET original_wake_id=NULL,wake_id=NULL",
                [],
            )
            .unwrap();
        assert!(render(s.conn(), &row, NOW).is_err());
    }
    #[test]
    fn human_wait_uses_existing_24_72_tiers_and_resolution_invalidates_old_fact() {
        let mut s = store();
        let p = plan(&mut s);
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.block(
                    &crewd_work::Actor::Human,
                    now,
                    WorkId(1),
                    vec![crewd_work::BlockOn::NewAsk {
                        question: "PRIVATE_HUMAN_QUESTION_NOT_RENDERED".into(),
                        default: None,
                    }],
                    crewd_work::Continuation::Nothing,
                )
            })
        });
        assert_eq!(ok(&mut s, enqueue), 0);
        assert_eq!(ok_at(&mut s, NOW + 24 * 60 * MIN, enqueue).0, 1);
        let first = captain_rows(&s)[0].clone();
        let work::FollowupNote::Current(body) =
            render(s.conn(), &first, NOW + 24 * 60 * MIN).unwrap()
        else {
            panic!("24h fact")
        };
        assert!(body.contains(&p));
        assert!(!body.contains("PRIVATE_HUMAN_QUESTION"));
        assert_eq!(ok_at(&mut s, NOW + 24 * 60 * MIN + MIN, enqueue).0, 0);
        assert_eq!(ok_at(&mut s, NOW + 72 * 60 * MIN, enqueue).0, 1);
        let rows = captain_rows(&s);
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            render(s.conn(), &first, NOW + 72 * 60 * MIN).unwrap(),
            work::FollowupNote::Stale
        ));
        let h = collect(s.conn(), NOW + 72 * 60 * MIN, false)
            .unwrap()
            .into_iter()
            .find_map(|f| match f.cause {
                Cause::HumanWait { ask, .. } => Some(ask),
                _ => None,
            })
            .unwrap();
        ok_at(&mut s, NOW + 72 * 60 * MIN, |tx| {
            asks::answer(tx, "crew-a", &h, "actual Human resolution")
        });
        assert_eq!(ok_at(&mut s, NOW + 72 * 60 * MIN + MIN, enqueue).0, 0);
        for row in rows {
            assert!(matches!(
                render(s.conn(), &row, NOW + 72 * 60 * MIN + MIN).unwrap(),
                work::FollowupNote::Stale
            ));
        }
    }
    #[test]
    fn unfinished_dependency_alone_is_not_actionable_but_verified_loss_is() {
        let mut s = store();
        let parent = plan(&mut s);
        let dependent = ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "private dependent".into(),
                    owner: Some("w-a2".into()),
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.block(
                    &crewd_work::Actor::Human,
                    now,
                    WorkId(2),
                    vec![crewd_work::BlockOn::Item(WorkId(1))],
                    crewd_work::Continuation::Nothing,
                )
            })
        });
        assert_eq!(ok(&mut s, enqueue), 0);
        failure(&s, "actual dependency failure");
        assert_eq!(ok(&mut s, enqueue), 1);
        let row = captain_rows(&s)[0].clone();
        let work::FollowupNote::Current(body) = render(s.conn(), &row, NOW).unwrap() else {
            panic!("verified loss")
        };
        assert!(body.contains(&parent) && body.contains(&dependent.id));
        assert!(body.contains("dependency_lost"));
        assert_eq!(projection(&s).len(), 2);
    }
    #[test]
    fn current_captain_changes_routing_and_archive_observation_does_not_revive() {
        let mut s = store();
        plan(&mut s);
        failure(&s, "actual owner failure");
        ok(&mut s, enqueue);
        let old = captain_rows(&s)[0].clone();
        ok(&mut s, |tx| crews::set_captain(tx, "crew-a", "w-a2"));
        assert!(matches!(
            render(s.conn(), &old, NOW).unwrap(),
            work::FollowupNote::Stale
        ));
        assert_eq!(ok(&mut s, enqueue), 1);
        assert_eq!(
            wakes::list(s.conn(), Some("w-a2"), 100)
                .unwrap()
                .into_iter()
                .filter(|r| r.source.starts_with("work-followup:"))
                .count(),
            1
        );
        let mut s = store();
        ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-x".into(),
                    title: "archive projection".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        });
        failure(&s, "retained source failure");
        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        assert_eq!(ok(&mut s, enqueue), 0);
        assert!(projection(&s).iter().all(|r| r.1 == "suppressed"));
        ok(&mut s, |tx| crews::unarchive(tx, "crew-x"));
        assert_eq!(ok(&mut s, enqueue), 0);
        failure(&s, "genuinely new failure after restore");
        assert_eq!(ok(&mut s, enqueue), 1);
    }
    #[test]
    fn actual_old_snapshot_restore_never_replays_same_fact_and_keeps_source_unchanged() {
        let mut s = store();
        plan(&mut s);
        failure(&s, "actual old snapshot cause");
        ok(&mut s, enqueue);
        let first = captain_rows(&s)[0].clone();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old-before-ack.db");
        s.conn()
            .execute("VACUUM INTO ?1", [source.to_str().unwrap()])
            .unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        ok(&mut s, |tx| wakes::claim(tx, &[first.id]));
        ok(&mut s, |tx| wakes::mark_delivered(tx, &[first.id]));
        assert_eq!(projection(&s)[0].1, "delivered");
        let dest = dir.path().join("restored");
        std::fs::create_dir(&dest).unwrap();
        crewd_store::backup::restore(&dest, &source).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        let mut restored = crewd_store::Store::open_data_dir(&dest).unwrap();
        assert!(durable::baseline_required(restored.conn()).unwrap());
        let old = wakes::by_ids(restored.conn(), &[first.id]).unwrap()[0].clone();
        assert_eq!(old.state, wakes::WakeState::Dropped);
        assert_eq!(projection(&restored)[0].1, "unknown");
        assert_eq!(projection(&restored)[0].3, Some(first.id));
        assert!(matches!(
            render(restored.conn(), &old, NOW).unwrap(),
            work::FollowupNote::Stale
        ));
        assert_eq!(ok(&mut restored, enqueue), 0);
        assert!(!durable::baseline_required(restored.conn()).unwrap());
        failure(&restored, "new actual cause after recovery baseline");
        assert_eq!(ok(&mut restored, enqueue), 1);
        let old_source = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            old_source
                .query_row("SELECT effect FROM work_followup_facts", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "enqueued"
        );
    }
}
