//! Real committed work events and reference backfill. BookState remains authoritative.
use crate::object_refs::{ObjectKind, ObjectReference};
use crate::{Result, StoreError};
use crewd_work::{BookState, Config, Event, WorkBook};
use rusqlite::{Connection, params};

fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}
/// Must run inside the same single-writer transaction as the book update.
pub fn append(conn: &Connection, crew: &str, events: &[Event]) -> Result<()> {
    if conn.is_autocommit() {
        return Err(invalid("work history append requires a transaction"));
    }
    for event in events {
        let seq = i64::try_from(event.seq).map_err(|_| invalid("work event sequence exhausted"))?;
        if seq <= 0 {
            return Err(invalid("work event sequence must be positive"));
        }
        let body = serde_json::to_string(event)?;
        // Duplicate dispatch cannot overwrite or counterfeit prior history.
        conn.execute(
            "INSERT INTO work_events(crew_id,seq,body) VALUES(?1,?2,?3)",
            params![crew, seq, body],
        )?;
    }
    Ok(())
}
pub fn list(conn: &Connection, crew: &str, after: u64, limit: usize) -> Result<Vec<Event>> {
    if limit == 0 || limit > 200 {
        return Err(invalid("work history limit must be 1..200"));
    }
    let after = i64::try_from(after).map_err(|_| invalid("work history sequence out of range"))?;
    let mut stmt = conn.prepare(
        "SELECT seq,body FROM work_events WHERE crew_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![crew, after, limit as i64], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, body) = row?;
        let event: Event = serde_json::from_str(&body)?;
        if event.seq != seq as u64 {
            return Err(invalid("work history row identity mismatch"));
        }
        out.push(event);
    }
    Ok(out)
}
// This is indexing of already-existing state, including removed crews. It is not
// an action that creates new work or bypasses the normal frozen-crew write rule.
fn existing_ref(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    source: &str,
    now: &str,
) -> Result<ObjectReference> {
    if let Some(r) = crate::object_refs::resolve(conn, kind, crew, source)? {
        return Ok(r);
    }
    conn.execute(
        "INSERT INTO object_refs(kind,crew_id,source_id,created_at) VALUES(?1,?2,?3,?4)",
        params![kind.key(), crew, source, now],
    )?;
    crate::object_refs::resolve(conn, kind, crew, source)?
        .ok_or_else(|| invalid("work reference backfill missing"))
}
/// Replay-safe metadata indexing; no fabricated historical events or state rewrite.
pub fn backfill_references(conn: &Connection) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT crew_id,state,updated_at FROM work_books ORDER BY crew_id")?;
    let books = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (crew, json, at) in books {
        let state: BookState = serde_json::from_str(&json)?;
        if state.crew.as_str() != crew {
            return Err(invalid("work reference backfill crew mismatch"));
        }
        let book = WorkBook::restore(state, Config::default())
            .map_err(|_| invalid("invalid work book during reference backfill"))?;
        for id in book.state().entries.keys() {
            existing_ref(
                conn,
                ObjectKind::Requirement,
                &crew,
                &format!("work:{id}"),
                &at,
            )?;
        }
        for id in book.state().items.keys() {
            existing_ref(conn, ObjectKind::Plan, &crew, &format!("work:{id}"), &at)?;
        }
        for id in book.state().asks.keys() {
            existing_ref(
                conn,
                ObjectKind::HumanTask,
                &crew,
                &format!("work:{id}"),
                &at,
            )?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use crewd_work::{Actor, CrewId, NewItem, Ts};
    fn setup(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO crews(id,title,created_at,updated_at) VALUES('a','A','t','t');",
        )
        .unwrap();
    }
    fn events() -> Vec<Event> {
        let mut b = WorkBook::new(CrewId::new("a"), Config::default());
        b.create_item(
            &Actor::worker("w"),
            Ts(100),
            NewItem::titled("synthetic").owned_by("w"),
        )
        .unwrap();
        b.take_log()
    }
    #[test]
    fn history_is_ordered_paginated_durable_and_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("db");
        let e = events();
        {
            let s = Store::open(&path).unwrap();
            setup(s.conn());
            let tx = s.conn().unchecked_transaction().unwrap();
            append(&tx, "a", &e).unwrap();
            tx.commit().unwrap();
            let tx = s.conn().unchecked_transaction().unwrap();
            assert!(append(&tx, "a", &e).is_err());
            tx.rollback().unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(list(s.conn(), "a", 0, 1).unwrap(), e);
        assert!(list(s.conn(), "a", e[0].seq, 1).unwrap().is_empty());
        assert!(list(s.conn(), "a", 0, 201).is_err());
    }
    #[test]
    fn failed_transaction_leaves_no_history() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            append(&tx, "a", &events()).unwrap();
        }
        assert!(list(s.conn(), "a", 0, 200).unwrap().is_empty());
    }
    #[test]
    fn v5_backfill_indexes_frozen_books_without_rewriting_or_inventing_events() {
        let s = crate::testing::open_legacy_store(5).unwrap();
        setup(s.conn());
        s.conn().execute_batch("INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('w','a','worker','codex','t');").unwrap();
        let mut b = WorkBook::new(CrewId::new("a"), Config::default());
        b.create_item(
            &Actor::worker("w"),
            Ts(100),
            NewItem::titled("existing").owned_by("w"),
        )
        .unwrap();
        let state = serde_json::to_string(b.state()).unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('a',?1,'original')",
                [&state],
            )
            .unwrap();
        s.conn()
            .execute_batch("UPDATE crews SET archived_at='frozen';")
            .unwrap();
        crate::schema::migrate(s.conn()).unwrap();
        let r = crate::object_refs::resolve(s.conn(), ObjectKind::Plan, "a", "work:W1")
            .unwrap()
            .unwrap();
        backfill_references(s.conn()).unwrap();
        assert_eq!(
            crate::object_refs::resolve(s.conn(), ObjectKind::Plan, "a", "work:W1").unwrap(),
            Some(r)
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT state FROM work_books WHERE crew_id='a'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            state
        );
        assert!(list(s.conn(), "a", 0, 200).unwrap().is_empty());
        assert!(
            crate::object_refs::bind(s.conn(), ObjectKind::Plan, "a", "work:W2", "now").is_err()
        );
    }
    #[test]
    fn invalid_old_book_rolls_back_the_v6_schema_and_keeps_v5() {
        let s = crate::testing::open_legacy_store(5).unwrap();
        setup(s.conn());
        let invalid =
            serde_json::to_string(WorkBook::new(CrewId::new("another"), Config::default()).state())
                .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('a',?1,'t')",
                [&invalid],
            )
            .unwrap();
        assert!(crate::schema::migrate(s.conn()).is_err());
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='work_events'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM object_refs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn history_pages_are_complete_and_identity_tampering_is_refused() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let mut b = WorkBook::new(CrewId::new("a"), Config::default());
        for title in ["first", "second"] {
            b.create_item(
                &Actor::worker("w"),
                Ts(100),
                NewItem::titled(title).owned_by("w"),
            )
            .unwrap();
        }
        let e = b.take_log();
        assert_eq!(e.len(), 2);
        let tx = s.conn().unchecked_transaction().unwrap();
        append(&tx, "a", &e).unwrap();
        tx.commit().unwrap();
        assert_eq!(list(s.conn(), "a", 0, 1).unwrap(), vec![e[0].clone()]);
        assert_eq!(
            list(s.conn(), "a", e[0].seq, 1).unwrap(),
            vec![e[1].clone()]
        );
        s.conn()
            .execute(
                "UPDATE work_events SET body=?1 WHERE seq=1",
                [serde_json::to_string(&e[1]).unwrap()],
            )
            .unwrap();
        assert!(list(s.conn(), "a", 0, 200).is_err());
    }
    #[test]
    fn append_requires_a_transaction_and_rejects_partial_conflicting_batches() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let e = events();
        assert!(append(s.conn(), "a", &e).is_err());
        let tx = s.conn().unchecked_transaction().unwrap();
        append(&tx, "a", &e).unwrap();
        tx.commit().unwrap();
        let mut newer = e[0].clone();
        newer.seq += 1;
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(append(&tx, "a", &[newer, e[0].clone()]).is_err());
        tx.rollback().unwrap();
        assert_eq!(list(s.conn(), "a", 0, 200).unwrap(), e);
    }
    #[test]
    fn a_later_invalid_book_rolls_back_earlier_reference_backfill() {
        let s = crate::testing::open_legacy_store(5).unwrap();
        setup(s.conn());
        s.conn()
            .execute_batch(
                "INSERT INTO crews(id,title,created_at,updated_at) VALUES('z','Z','t','t');",
            )
            .unwrap();
        let mut good = WorkBook::new(CrewId::new("a"), Config::default());
        good.create_item(
            &Actor::worker("w"),
            Ts(100),
            NewItem::titled("valid first").owned_by("w"),
        )
        .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('a',?1,'t')",
                [serde_json::to_string(good.state()).unwrap()],
            )
            .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('z','not json','t')",
                [],
            )
            .unwrap();
        assert!(crate::schema::migrate(s.conn()).is_err());
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM object_refs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
    }
}
