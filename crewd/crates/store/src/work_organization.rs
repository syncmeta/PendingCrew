//! Schema-9 organization authority, staged behind explicit migration activation.
//! One state row; object_refs.core_id is identity only. Old states become immutable evidence.
use crate::{Result, StoreError, object_refs::ObjectKind};
use crewd_work::{BookState, Config, Event, LegacyMerge, WorkBook};
use rusqlite::{Connection, OptionalExtension, params};

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}
fn transaction(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(invalid("organization work requires the caller transaction"));
    }
    Ok(())
}

/// Prepare only. It does not mutate schema or content and diagnoses the original crew/local handle.
pub fn prepare(conn: &Connection) -> Result<LegacyMerge> {
    let mut q = conn.prepare("SELECT crew_id,state FROM work_books ORDER BY crew_id")?;
    let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut books = Vec::new();
    for row in rows {
        let (crew, raw) = row?;
        let state: BookState = serde_json::from_str(&raw)?;
        if state.crew.as_str() != crew {
            return Err(invalid(format!(
                "legacy work crew {crew}: state identity mismatch"
            )));
        }
        books.push(state);
    }
    crewd_work::merge_legacy_books(books).map_err(|e| invalid(e.to_string()))
}

/// Must be invoked only by schema-9 migration after usage schema 8 is integrated.
/// This function deliberately does not set user_version or start/commit its own transaction.
pub fn migrate_in_tx(conn: &Connection, now: &str) -> Result<()> {
    transaction(conn)?;
    let initialized:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='organization_work_book')",[],|r|r.get(0))?;
    if initialized {
        // A retry must find valid authority and every published work identity.
        let state = load(conn)?
            .ok_or_else(|| invalid("organization authority is missing after migration"))?;
        verify_references(conn, &state)?;
        return Ok(());
    }
    let merged = prepare(conn)?;
    for item in merged.state.items.values() {
        if let Some(owner) = &item.owner
            && crate::sessions::get(conn, owner.as_str())?.is_none()
        {
            return Err(invalid(format!(
                "organization {}: owner session is missing",
                item.id
            )));
        }
    }

    // Verify every old issued reference against the exact crew/type identity map
    // before any schema change. Unknown issued objects cannot be silently recreated.
    let reference_rows = {
        let mut q=conn.prepare("SELECT number,kind,crew_id,source_id FROM object_refs WHERE substr(source_id,1,5)='work:' ORDER BY number")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut bindings = Vec::new();
    for (number, kind, crew, source) in reference_rows {
        let map = merged
            .identities
            .get(&crewd_work::CrewId::new(crew.clone()))
            .ok_or_else(|| invalid(format!("crew {crew}: issued {source} has no authority")))?;
        let local = &source[5..];
        let core = match kind.as_str() {
            "plan" => local
                .parse::<crewd_work::WorkId>()
                .ok()
                .and_then(|id| map.items.get(&id))
                .map(ToString::to_string),
            "human_task" => local
                .parse::<crewd_work::AskId>()
                .ok()
                .and_then(|id| map.asks.get(&id))
                .map(ToString::to_string),
            "requirement" => local
                .parse::<crewd_work::EntryNo>()
                .ok()
                .and_then(|id| map.entries.get(&id))
                .map(ToString::to_string),
            _ => None,
        }
        .ok_or_else(|| {
            invalid(format!(
                "crew {crew}: issued {source} has no mapped {kind} object"
            ))
        })?;
        bindings.push((number, core));
    }
    // Metadata for an existing human task may never outlive its authoritative question.
    let mut q = conn.prepare("SELECT crew_id,ask FROM ask_meta ORDER BY crew_id,ask")?;
    for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (crew, ask) = row?;
        if !merged
            .identities
            .get(&crewd_work::CrewId::new(crew.clone()))
            .is_some_and(|map| {
                ask.parse::<crewd_work::AskId>()
                    .is_ok_and(|id| map.asks.contains_key(&id))
            })
        {
            return Err(invalid(format!(
                "crew {crew}: ask metadata {ask} has no authority"
            )));
        }
    }
    drop(q);
    conn.execute_batch("CREATE TABLE organization_work_book(singleton INTEGER PRIMARY KEY CHECK(singleton=1),state TEXT NOT NULL,updated_at TEXT NOT NULL);
        CREATE TABLE organization_work_events(seq INTEGER PRIMARY KEY,crew_id TEXT NOT NULL,body TEXT NOT NULL);
        CREATE TABLE legacy_work_sequences(crew_id TEXT PRIMARY KEY,last_seq TEXT NOT NULL);
        ALTER TABLE object_refs ADD COLUMN core_id TEXT;
        CREATE UNIQUE INDEX object_refs_work_core ON object_refs(kind,core_id) WHERE core_id IS NOT NULL;")?;
    for (number, core) in bindings {
        conn.execute(
            "UPDATE object_refs SET core_id=?1 WHERE number=?2",
            params![core, number],
        )?;
    }
    verify_references(conn, &merged.state)?;
    // Stronger migration check: every authoritative old object already has its
    // published reference. No new registry number is manufactured during conversion.
    for (kind, ids) in [
        (
            ObjectKind::Plan,
            merged
                .state
                .items
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        ),
        (
            ObjectKind::HumanTask,
            merged
                .state
                .asks
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        ),
        (
            ObjectKind::Requirement,
            merged
                .state
                .entries
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        ),
    ] {
        for id in ids {
            if reference_for_core(conn, kind, &id)?.is_none() {
                return Err(invalid(format!(
                    "organization {id}: published identity is missing"
                )));
            }
        }
    }
    for (crew, seq) in merged.legacy_sequences {
        conn.execute(
            "INSERT INTO legacy_work_sequences(crew_id,last_seq) VALUES(?1,?2)",
            params![crew.to_string(), seq.to_string()],
        )?;
    }
    conn.execute(
        "INSERT INTO organization_work_book(singleton,state,updated_at) VALUES(1,?1,?2)",
        params![serde_json::to_string(&merged.state)?, now],
    )?;
    // Existing wake targets, sources and stop-check manifests are stable EXTERNAL
    // registry identities already. Rewriting those numbers would counterfeit old
    // receipt identities. Only legacy BookState IDs were translated above.
    conn.execute_batch("ALTER TABLE work_books RENAME TO legacy_work_books;
        CREATE TRIGGER legacy_work_books_no_insert BEFORE INSERT ON legacy_work_books BEGIN SELECT RAISE(ABORT,'legacy work authority is immutable'); END;
        CREATE TRIGGER legacy_work_books_no_update BEFORE UPDATE ON legacy_work_books BEGIN SELECT RAISE(ABORT,'legacy work authority is immutable'); END;
        CREATE TRIGGER legacy_work_books_no_delete BEFORE DELETE ON legacy_work_books BEGIN SELECT RAISE(ABORT,'legacy work authority is immutable'); END;
        CREATE VIEW work_books AS SELECT crew_id,state,updated_at FROM legacy_work_books;")?;
    Ok(())
}

pub fn load(conn: &Connection) -> Result<Option<BookState>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT state FROM organization_work_book WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let state: BookState = serde_json::from_str(&raw)?;
        if state.organization.is_none() {
            return Err(invalid("organization state has legacy shape"));
        }
        WorkBook::restore(state.clone(), Config::default())
            .map_err(|v| invalid(format!("organization state: {v}")))?;
        Ok(state)
    })
    .transpose()
}

pub fn core_for_reference(
    conn: &Connection,
    kind: ObjectKind,
    number: i64,
) -> Result<Option<String>> {
    let row: Option<Option<String>> = conn
        .query_row(
            "SELECT core_id FROM object_refs WHERE kind=?1 AND number=?2",
            params![kind.key(), number],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row.flatten())
}
pub fn reference_for_core(
    conn: &Connection,
    kind: ObjectKind,
    core: &str,
) -> Result<Option<crate::object_refs::ObjectReference>> {
    let number: Option<i64> = conn
        .query_row(
            "SELECT number FROM object_refs WHERE kind=?1 AND core_id=?2",
            params![kind.key(), core],
            |r| r.get(0),
        )
        .optional()?;
    number
        .map(|n| crate::object_refs::get(conn, kind, n))
        .transpose()
        .map(Option::flatten)
}
fn verify_references(conn: &Connection, state: &BookState) -> Result<()> {
    let mut q =
        conn.prepare("SELECT kind,crew_id,core_id FROM object_refs WHERE core_id IS NOT NULL")?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (kind, crew, core) = row?;
        let attributed = match kind.as_str() {
            "plan" => core.parse().ok().and_then(|id| state.item_crew(id)),
            "human_task" => core.parse().ok().and_then(|id| state.ask_crew(id)),
            "requirement" => core.parse().ok().and_then(|id| state.entry_crew(id)),
            _ => None,
        };
        if attributed.map(crewd_work::CrewId::as_str) != Some(crew.as_str()) {
            return Err(invalid(format!(
                "crew {crew}: issued core identity {core} has missing or foreign authority"
            )));
        }
    }
    for (kind, ids) in [
        (
            ObjectKind::Plan,
            state
                .items
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        ),
        (
            ObjectKind::HumanTask,
            state
                .asks
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        ),
        (
            ObjectKind::Requirement,
            state
                .entries
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        ),
    ] {
        for id in ids {
            if reference_for_core(conn, kind, &id)?.is_none() {
                return Err(invalid(format!(
                    "organization {id}: published identity is missing"
                )));
            }
        }
    }
    Ok(())
}

/// Call after binding each newly created object's registry identity. Writes only
/// the sole authority and its real new events inside the caller transaction.
pub fn save(conn: &Connection, state: &BookState, events: &[Event], now: &str) -> Result<()> {
    transaction(conn)?;
    if state.organization.is_none() {
        return Err(invalid(
            "cannot save legacy shape as organization authority",
        ));
    }
    WorkBook::restore(state.clone(), Config::default()).map_err(|v| invalid(v.to_string()))?;
    verify_references(conn, state)?;
    let before = load(conn)?.ok_or_else(|| invalid("organization authority is missing"))?;
    if state.last_item < before.last_item
        || state.last_ask < before.last_ask
        || state.last_seq < before.last_seq
        || before.items.keys().any(|id| !state.items.contains_key(id))
        || before.asks.keys().any(|id| !state.asks.contains_key(id))
        || before
            .entries
            .keys()
            .any(|id| !state.entries.contains_key(id))
    {
        return Err(invalid("organization write retired an issued identity"));
    }
    let mut seq = before.last_seq;
    for event in events {
        seq = seq
            .checked_add(1)
            .ok_or_else(|| invalid("organization event sequence exhausted"))?;
        if event.seq != seq {
            return Err(invalid("organization event sequence is not contiguous"));
        }
    }
    if seq != state.last_seq {
        return Err(invalid("organization write omitted real events"));
    }
    let org = state.organization.as_ref().expect("checked");
    for event in events {
        use crewd_work::EventKind::*;
        let crew = match &event.kind {
            EntryRegistered { entry } | EntryClaimed { entry, .. } | EntryClosed { entry, .. } => {
                org.entry_crews.get(entry)
            }
            AskOpened { ask, .. }
            | AskAnswered { ask, .. }
            | AskDefaulted { ask, .. }
            | AskWithdrawn { ask, .. }
            | EscalationNoted { ask, .. } => org.ask_crews.get(ask),
            ItemCreated { id, .. }
            | Reported { id, .. }
            | Done { id, .. }
            | Dropped { id, .. }
            | Relinked { id, .. }
            | ChannelLinked { id, .. }
            | Blocked { id, .. }
            | BlockerCleared { id, .. }
            | OwnerChanged { id, .. }
            | TransferRequested { id, .. }
            | TransferWithdrawn { id, .. }
            | CheckpointWake { id, .. } => org.item_crews.get(id),
        }
        .map(ToString::to_string)
        .ok_or_else(|| invalid("event has no object crew attribution"))?;
        let seq = i64::try_from(event.seq)
            .map_err(|_| invalid("organization event sequence exhausted"))?;
        conn.execute(
            "INSERT INTO organization_work_events(seq,crew_id,body) VALUES(?1,?2,?3)",
            params![seq, crew, serde_json::to_string(event)?],
        )?;
    }
    if conn.execute(
        "UPDATE organization_work_book SET state=?1,updated_at=?2 WHERE singleton=1",
        params![serde_json::to_string(state)?, now],
    )? != 1
    {
        return Err(invalid("organization authority disappeared during write"));
    }
    Ok(())
}

/// New objects receive a fresh public registry number without colliding with
/// a legacy work:<local> alias belonging to the same crew.
pub fn bind_core(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    core: &str,
    now: &str,
) -> Result<crate::object_refs::ObjectReference> {
    transaction(conn)?;
    let valid = match kind {
        ObjectKind::Plan => core
            .parse::<crewd_work::WorkId>()
            .is_ok_and(|id| id.0 > 0 && id.to_string() == core),
        ObjectKind::HumanTask => core
            .parse::<crewd_work::AskId>()
            .is_ok_and(|id| id.0 > 0 && id.to_string() == core),
        ObjectKind::Requirement => core
            .parse::<crewd_work::EntryNo>()
            .is_ok_and(|id| id.0 > 0 && id.to_string() == core),
        _ => false,
    };
    if !valid {
        return Err(invalid("invalid organization core identity"));
    }
    if !matches!(
        kind,
        ObjectKind::Plan | ObjectKind::HumanTask | ObjectKind::Requirement
    ) {
        return Err(invalid("unsupported organization core identity kind"));
    }
    if let Some(existing) = reference_for_core(conn, kind, core)? {
        if existing.crew_id != crew {
            return Err(invalid(
                "organization core identity belongs to another crew",
            ));
        }
        return Ok(existing);
    }
    let reference =
        crate::object_refs::bind(conn, kind, crew, &format!("organization:{core}"), now)?;
    if conn.execute(
        "UPDATE object_refs SET core_id=?1 WHERE number=?2 AND core_id IS NULL",
        params![core, reference.number],
    )? != 1
    {
        return Err(invalid("organization identity binding was already mapped"));
    }
    Ok(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use crewd_work::{Actor, CrewId, EntryNo, NewItem, Ts};
    fn setup(conn: &Connection) {
        conn.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('a','A','t','t'),('b','B','t','t');").unwrap();
        for crew in ["a", "b"] {
            let mut book = WorkBook::new(CrewId::new(crew), Config::default());
            book.register_entry(Ts(1000), EntryNo(1)).unwrap();
            book.create_item(
                &Actor::Human,
                Ts(1000),
                NewItem::titled(crew).from_entry(EntryNo(1)),
            )
            .unwrap();
            book.open_ask(&Actor::System, Ts(1000), "original ask", None, None)
                .unwrap();
            conn.execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,'t')",
                params![crew, serde_json::to_string(book.state()).unwrap()],
            )
            .unwrap();
            for (kind, id) in [
                (ObjectKind::Plan, "W1"),
                (ObjectKind::HumanTask, "H1"),
                (ObjectKind::Requirement, "#1"),
            ] {
                crate::object_refs::bind(conn, kind, crew, &format!("work:{id}"), "original")
                    .unwrap();
            }
        }
    }
    fn snapshot(conn: &Connection) -> Vec<(i64, String, String, String)> {
        let mut q = conn
            .prepare("SELECT number,kind,source_id,created_at FROM object_refs ORDER BY number")
            .unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    #[test]
    fn migration_preserves_public_ids_aliases_original_state_and_event_evidence() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let before = snapshot(s.conn());
        let originals: Vec<String> = {
            let mut q = s
                .conn()
                .prepare("SELECT state FROM work_books ORDER BY crew_id")
                .unwrap();
            q.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let tx = s.conn().unchecked_transaction().unwrap();
        migrate_in_tx(&tx, "migration").unwrap();
        tx.commit().unwrap();
        assert_eq!(snapshot(s.conn()), before);
        let state = load(s.conn()).unwrap().unwrap();
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.asks.len(), 2);
        assert_eq!(state.entries.len(), 2);
        for crew in ["a", "b"] {
            for (kind, id) in [
                (ObjectKind::Plan, "W1"),
                (ObjectKind::HumanTask, "H1"),
                (ObjectKind::Requirement, "#1"),
            ] {
                let r = crate::object_refs::resolve(s.conn(), kind, crew, &format!("work:{id}"))
                    .unwrap()
                    .unwrap();
                assert!(
                    core_for_reference(s.conn(), kind, r.number)
                        .unwrap()
                        .is_some()
                );
            }
        }
        let archived: Vec<String> = {
            let mut q = s
                .conn()
                .prepare("SELECT state FROM legacy_work_books ORDER BY crew_id")
                .unwrap();
            q.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(archived, originals);
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM legacy_work_sequences", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(
            s.conn()
                .execute("UPDATE work_books SET state='{}'", [])
                .is_err()
        );
        assert!(
            s.conn()
                .execute("UPDATE legacy_work_books SET state='{}'", [])
                .is_err()
        );
        let tx = s.conn().unchecked_transaction().unwrap();
        migrate_in_tx(&tx, "retry").unwrap();
        tx.commit().unwrap();
        assert_eq!(load(s.conn()).unwrap().unwrap(), state);
    }
    #[test]
    fn missing_issued_authority_aborts_before_schema_changes() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        crate::object_refs::bind(s.conn(), ObjectKind::Plan, "a", "work:W999", "t").unwrap();
        let before = snapshot(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            assert!(migrate_in_tx(&tx, "t").is_err());
        }
        assert_eq!(snapshot(s.conn()), before);
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    #[test]
    fn mid_migration_failure_rolls_back_schema_and_identity_updates() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let before = snapshot(s.conn());
        s.conn().execute_batch("CREATE TRIGGER inject_org_migration_failure BEFORE UPDATE ON object_refs WHEN old.crew_id='b' BEGIN SELECT RAISE(ABORT,'injected identity failure'); END;").unwrap();
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            assert!(migrate_in_tx(&tx, "t").is_err());
        }
        assert_eq!(snapshot(s.conn()), before);
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let columns: Vec<String> = {
            let mut q = s.conn().prepare("PRAGMA table_info(object_refs)").unwrap();
            q.query_map([], |r| r.get(1))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert!(!columns.contains(&"core_id".into()));
    }
    #[test]
    fn new_objects_get_fresh_numbers_and_real_events_in_the_same_transaction() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        let original = snapshot(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            let mut book =
                WorkBook::restore(load(&tx).unwrap().unwrap(), Config::default()).unwrap();
            let mut n = NewItem::titled("new");
            n.crew_id = Some(CrewId::new("a"));
            let made = book.create_item(&Actor::Human, Ts(2000), n).unwrap();
            let r = bind_core(&tx, ObjectKind::Plan, "a", &made.value.to_string(), "t").unwrap();
            assert!(r.number > original.last().unwrap().0);
            save(&tx, book.state(), &made.events, "t").unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(load(s.conn()).unwrap().unwrap().items.len(), 3);
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
        assert_eq!(snapshot(s.conn())[..original.len()], original);
    }
    #[test]
    fn dropped_transaction_discards_state_identity_and_events_together() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        let original = load(s.conn()).unwrap().unwrap();
        let before = snapshot(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            let mut b = WorkBook::restore(original.clone(), Config::default()).unwrap();
            let mut n = NewItem::titled("rollback");
            n.crew_id = Some(CrewId::new("a"));
            let made = b.create_item(&Actor::Human, Ts(2000), n).unwrap();
            bind_core(&tx, ObjectKind::Plan, "a", &made.value.to_string(), "t").unwrap();
            save(&tx, b.state(), &made.events, "t").unwrap();
        }
        assert_eq!(load(s.conn()).unwrap().unwrap(), original);
        assert_eq!(snapshot(s.conn()), before);
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
    }
    #[test]
    fn synthetic_pre_migration_backup_restores_original_authority_and_identities() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("active.db");
        let backup = dir.path().join("before.db");
        let store = Store::open(&path).unwrap();
        setup(store.conn());
        let before = snapshot(store.conn());
        crate::backup::ensure_ok(store.conn(), crate::backup::Check::Full).unwrap();
        crate::backup::vacuum_into(store.conn(), &backup).unwrap();
        {
            let tx = store.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        crate::backup::ensure_ok(store.conn(), crate::backup::Check::Full).unwrap();
        let restored = Store::open(&backup).unwrap();
        assert_eq!(snapshot(restored.conn()), before);
        assert_eq!(
            restored
                .conn()
                .query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            restored
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let retried = prepare(restored.conn()).unwrap();
        assert_eq!(retried.state, load(store.conn()).unwrap().unwrap());
    }
}
