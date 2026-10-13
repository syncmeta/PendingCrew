//! Per-session operational stop-check budget. It never copies work status.
use crate::{Result, StoreError};
use crewd_work::{BookState, Config, StopStreak, WorkBook};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    pub epoch: i64,
    pub streak: StopStreak,
}
fn invalid(s: &str) -> StoreError {
    StoreError::Invalid(s.into())
}
fn validate(s: &StopStreak) -> Result<()> {
    let cfg = Config::default();
    if s.unimproved > cfg.stop_check_limit || s.rewakes > cfg.stop_rewake_budget {
        return Err(invalid("stop-check budget exceeds configured bounds"));
    }
    Ok(())
}
pub fn get(conn: &Connection, session: &str) -> Result<Option<Budget>> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT epoch,state FROM work_stop_checks WHERE session_id=?1",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(epoch, state)| {
        let streak: StopStreak = serde_json::from_str(&state)?;
        validate(&streak)?;
        Ok(Budget { epoch, streak })
    })
    .transpose()
}
/// None is an absent row, distinct from a migrated row at epoch zero.
pub fn save(
    conn: &Connection,
    session: &str,
    expected: Option<i64>,
    streak: &StopStreak,
) -> Result<bool> {
    if conn.is_autocommit() {
        return Err(invalid("stop-check budget save requires a transaction"));
    }
    validate(streak)?;
    let body = serde_json::to_string(streak)?;
    let changed=match expected {
        None=>conn.execute("INSERT INTO work_stop_checks(session_id,epoch,state) VALUES(?1,1,?2) ON CONFLICT(session_id) DO NOTHING",params![session,body])?,
        Some(old)=>{
            if old<0 {return Err(invalid("invalid stop-check budget epoch"));}
            let next=old.checked_add(1).ok_or_else(||invalid("stop-check budget epoch exhausted"))?;
            conn.execute("UPDATE work_stop_checks SET epoch=?1,state=?2 WHERE session_id=?3 AND epoch=?4",params![next,body,session,old])?
        }
    };
    Ok(changed == 1)
}
/// Preserve any legacy per-book counters, conservatively merging observations
/// with maxima rather than counting one shared idle edge once per crew.
pub(crate) fn backfill(conn: &Connection) -> Result<()> {
    let rows = {
        let mut q = conn.prepare("SELECT crew_id,state FROM work_books ORDER BY crew_id")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut sessions: BTreeMap<String, StopStreak> = BTreeMap::new();
    for (crew, raw) in rows {
        let state: BookState = serde_json::from_str(&raw)?;
        if state.crew.as_str() != crew {
            return Err(invalid("stop-check backfill crew identity mismatch"));
        }
        WorkBook::restore(state.clone(), Config::default())
            .map_err(|_| invalid("stop-check backfill book is invalid"))?;
        for (session, mut streak) in state.stop_streaks {
            validate(&streak)?;
            if crate::sessions::get(conn, session.as_str())?.is_none() {
                continue;
            }
            for violation in &mut streak.last {
                let r = crate::object_refs::resolve(
                    conn,
                    crate::object_refs::ObjectKind::Plan,
                    &crew,
                    &format!("work:{}", violation.item),
                )?
                .ok_or_else(|| invalid("legacy stop-check violation has no issued identity"))?;
                violation.item = r
                    .id()
                    .parse()
                    .map_err(|_| invalid("legacy stop-check global identity is invalid"))?;
            }
            let all = sessions.entry(session.to_string()).or_default();
            all.unimproved = all.unimproved.max(streak.unimproved);
            all.rewakes = all.rewakes.max(streak.rewakes);
            all.escalated |= streak.escalated;
            all.last.extend(streak.last);
        }
    }
    for (session, mut streak) in sessions {
        streak.last.sort_by_key(|v| (v.item, v.kind));
        streak.last.dedup();
        conn.execute(
            "INSERT INTO work_stop_checks(session_id,epoch,state) VALUES(?1,0,?2)",
            params![session, serde_json::to_string(&streak)?],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Store,
        object_refs::{self, ObjectKind},
    };
    use crewd_proto::{Crew, Session};
    use crewd_work::{Actor, CrewId, NewItem, Ts};
    fn seed(s: &Store) {
        for crew in ["a", "b"] {
            crate::crews::upsert(
                s.conn(),
                &Crew {
                    id: crew.into(),
                    title: crew.into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        crate::sessions::upsert(
            s.conn(),
            &Session {
                id: "worker".into(),
                crew_id: "a".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    #[test]
    fn budget_cas_is_transactional_durable_and_does_not_reuse_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let s = Store::open(&path).unwrap();
            seed(&s);
            assert!(save(s.conn(), "worker", None, &StopStreak::default()).is_err());
            let tx = s.conn().unchecked_transaction().unwrap();
            let state = StopStreak {
                rewakes: 2,
                ..Default::default()
            };
            assert!(save(&tx, "worker", None, &state).unwrap());
            assert!(!save(&tx, "worker", None, &state).unwrap());
            tx.commit().unwrap();
            let tx = s.conn().unchecked_transaction().unwrap();
            assert!(save(&tx, "worker", Some(1), &StopStreak::default()).unwrap());
            assert!(!save(&tx, "worker", Some(1), &state).unwrap());
            tx.commit().unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(
            get(s.conn(), "worker").unwrap(),
            Some(Budget {
                epoch: 2,
                streak: StopStreak::default()
            })
        );
        s.conn()
            .execute(
                "UPDATE work_stop_checks SET epoch=?1 WHERE session_id='worker'",
                [i64::MAX],
            )
            .unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(save(&tx, "worker", Some(i64::MAX), &StopStreak::default()).is_err());
        tx.rollback().unwrap();
        assert_eq!(get(s.conn(), "worker").unwrap().unwrap().epoch, i64::MAX);
        s.conn()
            .execute("DELETE FROM sessions WHERE id='worker'", [])
            .unwrap();
        assert!(get(s.conn(), "worker").unwrap().is_none());
    }
    fn legacy(s: &Store, crew: &str) -> String {
        let mut b = WorkBook::new(CrewId::new(crew), Config::default());
        let w = b
            .create_item(
                &Actor::Human,
                Ts(1),
                NewItem::titled("legacy work").owned_by("worker"),
            )
            .unwrap()
            .value;
        b.on_idle(&crewd_work::SessionId::new("worker"), Ts(1));
        b.on_idle(&crewd_work::SessionId::new("worker"), Ts(1));
        let raw = serde_json::to_string(b.state()).unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,'t')",
                params![crew, raw],
            )
            .unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        object_refs::bind(&tx, ObjectKind::Plan, crew, &format!("work:{w}"), "t").unwrap();
        tx.commit().unwrap();
        raw
    }
    #[test]
    fn v6_migration_preserves_global_budget_without_double_counting_crews() {
        let s = Store::open_in_memory().unwrap();
        seed(&s);
        let a = legacy(&s, "a");
        let b = legacy(&s, "b");
        let count: i64 = s
            .conn()
            .query_row("SELECT count(*) FROM object_refs", [], |r| r.get(0))
            .unwrap();
        s.conn()
            .execute_batch(
                "DROP TABLE runner_usage; DROP TABLE work_stop_checks; PRAGMA user_version=6;",
            )
            .unwrap();
        crate::schema::migrate(s.conn()).unwrap();
        let budget = get(s.conn(), "worker").unwrap().unwrap();
        assert_eq!(budget.epoch, 0);
        assert_eq!(budget.streak.rewakes, 2);
        assert_eq!(budget.streak.unimproved, 1);
        assert_eq!(budget.streak.last.len(), 2);
        assert_ne!(
            budget.streak.last[0].item, budget.streak.last[1].item,
            "same local W1 must remain two global identities"
        );
        for (crew, raw) in [("a", a), ("b", b)] {
            let current: String = s
                .conn()
                .query_row(
                    "SELECT state FROM work_books WHERE crew_id=?1",
                    [crew],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(current, raw);
        }
        assert_eq!(
            s.conn()
                .query_row::<i64, _, _>("SELECT count(*) FROM object_refs", [], |r| r.get(0))
                .unwrap(),
            count
        );
        assert_eq!(
            s.conn()
                .query_row::<i64, _, _>("SELECT count(*) FROM work_events", [], |r| r.get(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn bad_later_book_rolls_back_table_version_and_budget_migration() {
        let s = Store::open_in_memory().unwrap();
        seed(&s);
        legacy(&s, "a");
        let bad =
            serde_json::to_string(WorkBook::new(CrewId::new("wrong"), Config::default()).state())
                .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('b',?1,'t')",
                [bad],
            )
            .unwrap();
        s.conn()
            .execute_batch(
                "DROP TABLE runner_usage; DROP TABLE work_stop_checks; PRAGMA user_version=6;",
            )
            .unwrap();
        assert!(crate::schema::migrate(s.conn()).is_err());
        assert_eq!(
            s.conn()
                .query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            6
        );
        assert_eq!(
            s.conn()
                .query_row::<i64, _, _>(
                    "SELECT count(*) FROM sqlite_master WHERE name='work_stop_checks'",
                    [],
                    |r| r.get(0)
                )
                .unwrap(),
            0
        );
    }
}
