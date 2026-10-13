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
    let known_crews: std::collections::BTreeSet<String> = {
        let mut q = conn.prepare("SELECT id FROM crews")?;
        q.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut books = Vec::new();
    for row in rows {
        let (crew, raw) = row?;
        if !known_crews.contains(&crew) {
            return Err(invalid(format!(
                "legacy work crew {crew} has no original crew source"
            )));
        }
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
    let mut merged = prepare(conn)?;
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

    // Reconcile raw agent requirements with existing core entries by exact
    // crew + original local number, never by an organization-wide naked #1.
    let raw_rows = {
        let mut q=conn.prepare("SELECT id,crew_id,number,created_at FROM todos WHERE ledger='agent' ORDER BY crew_id,number")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut raw_bindings = Vec::new();
    let mut last_entry = merged.state.last_entry;
    let known_crews: std::collections::BTreeSet<String> = {
        let mut q = conn.prepare("SELECT id FROM crews")?;
        q.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (raw_id, crew, local, created) in raw_rows {
        if !known_crews.contains(&crew) {
            return Err(invalid(format!(
                "raw requirement {raw_id} has no original crew source"
            )));
        }
        if local <= 0 {
            return Err(invalid(format!(
                "crew {crew}: raw requirement number must be positive"
            )));
        }
        let existing = merged
            .identities
            .get(&crewd_work::CrewId::new(crew.clone()))
            .and_then(|map| map.entries.get(&crewd_work::EntryNo(local)))
            .copied();
        let core = if let Some(core) = existing {
            core
        } else {
            last_entry = last_entry
                .checked_add(1)
                .ok_or_else(|| invalid("organization requirement identity range exhausted"))?;
            merged.state.last_entry = last_entry;
            let core = crewd_work::EntryNo(last_entry);
            let registered_at = chrono::DateTime::parse_from_rfc3339(&created)
                .map_err(|_| {
                    invalid(format!(
                        "crew {crew}: raw requirement {raw_id} has invalid original time"
                    ))
                })?
                .timestamp_millis();
            merged.state.entries.insert(
                core,
                crewd_work::EntryTodo {
                    no: core,
                    registered_at: crewd_work::Ts(registered_at),
                    closed: None,
                },
            );
            merged
                .state
                .organization
                .as_mut()
                .expect("prepared")
                .entry_crews
                .insert(core, crewd_work::CrewId::new(crew.clone()));
            core
        };
        raw_bindings.push((raw_id, crew, core, created));
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
        ALTER TABLE wakes ADD COLUMN legacy_ask_alias TEXT;
        ALTER TABLE object_refs ADD COLUMN core_id TEXT;
        ALTER TABLE object_refs ADD COLUMN raw_todo_id TEXT REFERENCES todos(id);
        CREATE UNIQUE INDEX object_refs_raw_todo ON object_refs(raw_todo_id) WHERE raw_todo_id IS NOT NULL;
        CREATE UNIQUE INDEX object_refs_work_core ON object_refs(kind,core_id) WHERE core_id IS NOT NULL;")?;
    let old_ask_wakes = {
        let mut q = conn.prepare("SELECT id,source FROM wakes WHERE kind='ask_answer'")?;
        q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, source) in old_ask_wakes {
        if let Some((_, local)) = source.strip_prefix("ask:").and_then(|s| s.rsplit_once('/'))
            && let Ok(local) = local.parse::<crewd_work::AskId>()
        {
            conn.execute(
                "UPDATE wakes SET legacy_ask_alias=?1 WHERE id=?2",
                params![format!("work:{local}"), id],
            )?;
        }
    }
    for (number, core) in bindings {
        conn.execute(
            "UPDATE object_refs SET core_id=?1 WHERE number=?2",
            params![core, number],
        )?;
    }
    for (raw_id, crew, core, created) in raw_bindings {
        let number = if let Some(r) =
            reference_for_core(conn, ObjectKind::Requirement, &core.to_string())?
        {
            r.number
        } else {
            conn.execute("INSERT INTO object_refs(kind,crew_id,source_id,created_at,core_id) VALUES('requirement',?1,?2,?3,?4)",params![crew,format!("todo:{raw_id}"),created,core.to_string()])?;
            conn.last_insert_rowid()
        };
        if conn.execute("UPDATE object_refs SET raw_todo_id=?1 WHERE number=?2 AND kind='requirement' AND crew_id=?3 AND core_id=?4 AND raw_todo_id IS NULL",params![raw_id,number,crew,core.to_string()])?!=1 {
            return Err(invalid("raw requirement identity is duplicated or has mismatched source"));
        }
    }
    verify_references(conn, &merged.state)?;
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
        CREATE TRIGGER legacy_work_books_no_delete BEFORE DELETE ON legacy_work_books WHEN EXISTS(SELECT 1 FROM crews WHERE id=old.crew_id) BEGIN SELECT RAISE(ABORT,'legacy work authority is immutable'); END;
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
pub fn verify_references(conn: &Connection, state: &BookState) -> Result<()> {
    let mut identities = std::collections::BTreeSet::new();
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
        identities.insert((kind, core));
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
            if !identities.contains(&(kind.key().to_string(), id.clone())) {
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
        || state.last_entry < before.last_entry
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

/// New source fact is registered in the same insert savepoint as the raw Todo.
/// This stores lifecycle only in the sole authority; raw status is never copied.
pub fn register_raw_entry(conn: &Connection, todo: &crewd_proto::Todo) -> Result<()> {
    if todo.ledger != crewd_proto::Ledger::Agent {
        return Ok(());
    }
    let active:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='organization_work_book')",[],|r|r.get(0))?;
    if !active {
        return Ok(());
    } // pre-schema9 legacy import is reconciled by migration
    transaction(conn)?;
    let state =
        load(conn)?.ok_or_else(|| invalid("raw requirement has no organization authority"))?;
    let mut book =
        WorkBook::restore(state, Config::default()).map_err(|v| invalid(v.to_string()))?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT core_id FROM object_refs WHERE raw_todo_id=?1 AND kind='requirement'",
            [&todo.id],
            |r| r.get(0),
        )
        .optional()?;
    if existing.is_some() {
        return Ok(());
    }
    let last = book.state().last_entry;
    let core = crewd_work::EntryNo(
        last.checked_add(1)
            .ok_or_else(|| invalid("organization requirement identity range exhausted"))?,
    );
    let created = chrono::DateTime::parse_from_rfc3339(&todo.created_at)
        .map_err(|_| invalid("raw requirement has invalid original time"))?
        .timestamp_millis();
    let made = book
        .register_entry_in(
            crewd_work::Ts(created),
            core,
            crewd_work::CrewId::new(&todo.crew_id),
        )
        .map_err(|r| invalid(r.to_string()))?;
    let reference = crate::object_refs::bind(
        conn,
        ObjectKind::Requirement,
        &todo.crew_id,
        &format!("todo:{}", todo.id),
        &todo.created_at,
    )?;
    conn.execute(
        "UPDATE object_refs SET core_id=?1,raw_todo_id=?2 WHERE number=?3 AND core_id IS NULL",
        params![core.to_string(), todo.id, reference.number],
    )?;
    save(conn, book.state(), &made.events, &todo.created_at)
}

/// This destructive operation is called only after the existing human purge
/// preflight/snapshot and foreign-link guard, within the crew deletion transaction.
/// Registry/todo rows already cascade with the real crew; other crew nodes remain.
pub fn purge_crew_in_tx(conn: &Connection, crew: &str) -> Result<()> {
    transaction(conn)?;
    let mut state = load(conn)?.ok_or_else(|| invalid("purge has no organization authority"))?;
    let org = state.organization.as_mut().expect("organization load");
    let items: std::collections::BTreeSet<_> = org
        .item_crews
        .iter()
        .filter(|(_, c)| c.as_str() == crew)
        .map(|(id, _)| *id)
        .collect();
    let asks: std::collections::BTreeSet<_> = org
        .ask_crews
        .iter()
        .filter(|(_, c)| c.as_str() == crew)
        .map(|(id, _)| *id)
        .collect();
    let entries: std::collections::BTreeSet<_> = org
        .entry_crews
        .iter()
        .filter(|(_, c)| c.as_str() == crew)
        .map(|(id, _)| *id)
        .collect();
    org.item_crews.retain(|id, _| !items.contains(id));
    org.ask_crews.retain(|id, _| !asks.contains(id));
    org.entry_crews.retain(|id, _| !entries.contains(id));
    state.items.retain(|id, _| !items.contains(id));
    state.asks.retain(|id, _| !asks.contains(id));
    state.entries.retain(|id, _| !entries.contains(id));
    for streak in state.stop_streaks.values_mut() {
        streak.last.retain(|v| !items.contains(&v.item));
    }
    let surviving_sessions: std::collections::BTreeSet<String> = {
        let mut q = conn.prepare("SELECT id FROM sessions")?;
        q.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    state
        .stop_streaks
        .retain(|sid, _| surviving_sessions.contains(sid.as_str()));
    WorkBook::restore(state.clone(), Config::default())
        .map_err(|v| invalid(format!("purge would break organization graph: {v}")))?;
    verify_references(conn, &state)?;
    conn.execute(
        "UPDATE organization_work_book SET state=?1 WHERE singleton=1",
        [serde_json::to_string(&state)?],
    )?;
    conn.execute(
        "DELETE FROM organization_work_events WHERE crew_id=?1",
        [crew],
    )?;
    conn.execute("DELETE FROM legacy_work_sequences WHERE crew_id=?1", [crew])?;
    Ok(())
}

/// Genuine latest request for one organization node. Legacy archive events are not relabelled.
pub fn latest_transfer_request(
    conn: &Connection,
    crew: &str,
    id: crewd_work::WorkId,
) -> Result<Option<Event>> {
    let row:Option<(i64,String)>=conn.query_row("SELECT seq,body FROM organization_work_events WHERE crew_id=?1 AND json_extract(body,'$.kind.type')='transfer_requested' AND json_extract(body,'$.kind.id')=?2 ORDER BY seq DESC LIMIT 1",params![crew,id.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(seq, body)| {
        let event: Event = serde_json::from_str(&body)?;
        if seq <= 0
            || event.seq != seq as u64
            || !matches!(event.kind,crewd_work::EventKind::TransferRequested{id:i,..} if i==id)
        {
            return Err(invalid("transfer request journal identity mismatch"));
        }
        Ok(event)
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let s = crate::testing::open_v8_store().unwrap();
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
        let s = crate::testing::open_v8_store().unwrap();
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
        let s = crate::testing::open_v8_store().unwrap();
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
        let s = crate::testing::open_v8_store().unwrap();
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
        let s = crate::testing::open_v8_store().unwrap();
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
        let backup = dir.path().join("before.db");
        let store = crate::testing::open_v8_store().unwrap();
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
        let restored = crate::backup::open_read_only(&backup).unwrap();
        assert_eq!(snapshot(&restored), before);
        assert_eq!(
            restored
                .query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            restored
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let retried = prepare(&restored).unwrap();
        assert_eq!(retried.state, load(store.conn()).unwrap().unwrap());
    }
    fn raw(conn: &Connection, id: &str, crew: &str, number: i64) -> crewd_proto::Todo {
        let todo = crewd_proto::Todo {
            requirement_id: None,
            id: id.into(),
            crew_id: crew.into(),
            ledger: crewd_proto::Ledger::Agent,
            number,
            text: format!("original {id}"),
            status: "done".into(),
            created_at: "2026-10-13T00:00:00Z".into(),
            updated_at: None,
            deleted_at: None,
            attachments: vec![],
            responses: vec![],
        };
        crate::todos::insert(conn, &todo, None).unwrap();
        todo
    }
    #[test]
    fn raw_registered_source_reuses_exact_public_identity_and_unregistered_source_is_unique() {
        let s = crate::testing::open_v8_store().unwrap();
        setup(s.conn());
        let a = raw(s.conn(), "raw-a", "a", 1);
        let b = raw(s.conn(), "raw-b", "b", 2);
        let old = crate::object_refs::resolve(s.conn(), ObjectKind::Requirement, "a", "work:#1")
            .unwrap()
            .unwrap();
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        let a_view = crate::todos::list(s.conn(), "a", crewd_proto::Ledger::Agent)
            .unwrap()
            .remove(0);
        let b_view = crate::todos::list(s.conn(), "b", crewd_proto::Ledger::Agent)
            .unwrap()
            .remove(0);
        assert_eq!(a_view.requirement_id, Some(old.id()));
        assert_eq!(a_view.status, "in_progress");
        assert_eq!((a_view.number, a_view.text), (a.number, a.text));
        assert_eq!(b_view.status, "unclaimed");
        assert_ne!(a_view.requirement_id, b_view.requirement_id);
        assert_eq!((b_view.number, b_view.text), (b.number, b.text));
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM object_refs WHERE raw_todo_id='raw-a'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
    #[test]
    fn raw_source_state_is_derived_from_cross_crew_plans_in_the_organization_authority() {
        let s = crate::testing::open_v8_store().unwrap();
        setup(s.conn());
        raw(s.conn(), "raw-new", "a", 2);
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        let core: String = s
            .conn()
            .query_row(
                "SELECT core_id FROM object_refs WHERE raw_todo_id='raw-new'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let core = core.parse().unwrap();
        let made_id = {
            let tx = s.conn().unchecked_transaction().unwrap();
            let mut book =
                WorkBook::restore(load(&tx).unwrap().unwrap(), Config::default()).unwrap();
            let mut n = NewItem::titled("B implements A source").from_entry(core);
            n.crew_id = Some(CrewId::new("b"));
            let made = book
                .create_item(&Actor::Human, Ts(1_792_000_000_000), n)
                .unwrap();
            bind_core(&tx, ObjectKind::Plan, "b", &made.value.to_string(), "t").unwrap();
            save(&tx, book.state(), &made.events, "t").unwrap();
            tx.commit().unwrap();
            made.value
        };
        assert_eq!(
            crate::todos::list(s.conn(), "a", crewd_proto::Ledger::Agent).unwrap()[0].status,
            "in_progress"
        );
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            let mut book =
                WorkBook::restore(load(&tx).unwrap().unwrap(), Config::default()).unwrap();
            let done = book
                .mark_done(&Actor::Human, Ts(1_792_000_000_000), made_id, None)
                .unwrap();
            save(&tx, book.state(), &done.events, "t").unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            crate::todos::list(s.conn(), "a", crewd_proto::Ledger::Agent).unwrap()[0].status,
            "done"
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT status FROM todos WHERE id='raw-new'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "done"
        ); // old source fact was never lifecycle authority
    }
    #[test]
    fn new_raw_insert_is_registered_once_and_authority_failure_rolls_back_source_too() {
        let s = crate::testing::open_v8_store().unwrap();
        setup(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        let t = raw(s.conn(), "new-source", "a", 7);
        let view = crate::todos::list(s.conn(), "a", crewd_proto::Ledger::Agent)
            .unwrap()
            .remove(0);
        assert_eq!(view.status, "unclaimed");
        assert_eq!(view.number, 7);
        assert!(view.requirement_id.is_some());
        let before = load(s.conn()).unwrap().unwrap();
        let refs = snapshot(s.conn());
        s.conn().execute_batch("CREATE TRIGGER injected_raw_authority_failure BEFORE UPDATE ON organization_work_book BEGIN SELECT RAISE(ABORT,'injected raw state failure'); END;").unwrap();
        let mut bad = t;
        bad.id = "failed-source".into();
        bad.number = 8;
        assert!(crate::todos::insert(s.conn(), &bad, None).is_err());
        assert_eq!(load(s.conn()).unwrap().unwrap(), before);
        assert_eq!(snapshot(s.conn()), refs);
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM todos WHERE id='failed-source'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    #[test]
    fn orphan_raw_crew_refuses_migration_before_any_authority_or_mapping_write() {
        let s = crate::testing::open_v8_store().unwrap();
        setup(s.conn());
        s.conn().execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        raw(s.conn(), "orphan-raw", "missing-crew", 1);
        s.conn().execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        let refs = snapshot(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            assert!(
                migrate_in_tx(&tx, "t")
                    .unwrap_err()
                    .to_string()
                    .contains("no original crew source")
            );
        }
        assert_eq!(snapshot(s.conn()), refs);
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
    fn duplicate_original_local_requirement_number_is_rejected_without_second_object() {
        let s = crate::testing::open_v8_store().unwrap();
        setup(s.conn());
        let original = raw(s.conn(), "original-source", "a", 7);
        let mut duplicate = original;
        duplicate.id = "different-id-same-local-source".into();
        assert!(crate::todos::insert(s.conn(), &duplicate, None).is_err());
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM todos WHERE crew_id='a' AND ledger='agent' AND number=7",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            migrate_in_tx(&tx, "t").unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM object_refs WHERE raw_todo_id IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}
