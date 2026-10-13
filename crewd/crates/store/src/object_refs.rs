//! One global identity registry, not a second copy of work state.
//! Bindings are written inside the caller's single-writer transaction. Existing
//! per-crew source IDs are retained as aliases; public numbers never encode a provider.
use crate::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Requirement,
    Plan,
    HumanTask,
    Decision,
    Lesson,
    Risk,
    CustomRow,
    Table,
}
impl ObjectKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Requirement => "requirement",
            Self::Plan => "plan",
            Self::HumanTask => "human_task",
            Self::Decision => "decision",
            Self::Lesson => "lesson",
            Self::Risk => "risk",
            Self::CustomRow => "custom_row",
            Self::Table => "table",
        }
    }
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Requirement => "#",
            Self::Plan => "W",
            Self::HumanTask => "H",
            Self::Decision => "D",
            Self::Lesson => "L",
            Self::Risk => "R",
            Self::CustomRow => "O",
            Self::Table => "T",
        }
    }
}
// Transport DTOs must use id() as text, not expose an i64 as a JavaScript number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectReference {
    pub number: i64,
    pub kind: ObjectKind,
    pub crew_id: String,
    pub source_id: String,
    pub created_at: String,
}
impl ObjectReference {
    pub fn id(&self) -> String {
        format!("{}{}", self.kind.prefix(), self.number)
    }
}
fn read(r: &rusqlite::Row<'_>, kind: ObjectKind) -> rusqlite::Result<ObjectReference> {
    Ok(ObjectReference {
        number: r.get(0)?,
        kind,
        crew_id: r.get(1)?,
        source_id: r.get(2)?,
        created_at: r.get(3)?,
    })
}
pub fn resolve(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    source: &str,
) -> Result<Option<ObjectReference>> {
    Ok(conn.query_row("SELECT number,crew_id,source_id,created_at FROM object_refs WHERE kind=?1 AND crew_id=?2 AND source_id=?3",
        params![kind.key(),crew,source], |r|read(r,kind)).optional()?)
}
pub fn get(conn: &Connection, kind: ObjectKind, number: i64) -> Result<Option<ObjectReference>> {
    Ok(conn.query_row("SELECT number,crew_id,source_id,created_at FROM object_refs WHERE kind=?1 AND number=?2",
        params![kind.key(),number], |r|read(r,kind)).optional()?)
}
pub fn bind(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    source: &str,
    now: &str,
) -> Result<ObjectReference> {
    if source.trim().is_empty()
        || source.trim() != source
        || source.len() > 512
        || source.chars().any(char::is_control)
        || now.trim().is_empty()
    {
        return Err(StoreError::Invalid(
            "invalid object reference source or timestamp".into(),
        ));
    }
    if let Some(existing) = resolve(conn, kind, crew, source)? {
        return Ok(existing);
    }
    let removed: Option<Option<String>> = conn
        .query_row("SELECT archived_at FROM crews WHERE id=?1", [crew], |r| {
            r.get(0)
        })
        .optional()?;
    match removed {
        Some(None) => {}
        _ => {
            return Err(StoreError::Invalid(
                "object reference crew is missing or removed".into(),
            ));
        }
    }
    conn.execute(
        "INSERT INTO object_refs(kind,crew_id,source_id,created_at) VALUES(?1,?2,?3,?4)",
        params![kind.key(), crew, source, now],
    )?;
    resolve(conn, kind, crew, source)?
        .ok_or_else(|| StoreError::Invalid("object reference binding missing after insert".into()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    fn setup(conn: &Connection) {
        conn.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('a','A','t','t'),('b','B','t','t');").unwrap();
    }
    #[test]
    fn global_identity_is_idempotent_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let first = {
            let s = Store::open(&path).unwrap();
            setup(s.conn());
            let a = bind(s.conn(), ObjectKind::HumanTask, "a", "H1", "original").unwrap();
            let b = bind(s.conn(), ObjectKind::HumanTask, "b", "H1", "original").unwrap();
            let w = bind(s.conn(), ObjectKind::Plan, "a", "W1", "original").unwrap();
            assert_ne!(a.number, b.number);
            assert_ne!(a.number, w.number);
            assert_eq!(
                bind(s.conn(), ObjectKind::HumanTask, "a", "H1", "later").unwrap(),
                a
            );
            assert_eq!(a.id(), format!("H{}", a.number));
            a
        };
        let s = Store::open(&path).unwrap();
        assert_eq!(
            resolve(s.conn(), ObjectKind::HumanTask, "a", "H1").unwrap(),
            Some(first)
        );
    }
    #[test]
    fn purge_does_not_reuse_the_highest_global_number() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let a = bind(s.conn(), ObjectKind::Plan, "a", "W1", "t").unwrap();
        let b = bind(s.conn(), ObjectKind::HumanTask, "b", "H1", "t").unwrap();
        assert!(b.number > a.number);
        s.conn()
            .execute("DELETE FROM crews WHERE id='b'", [])
            .unwrap();
        assert!(
            get(s.conn(), ObjectKind::HumanTask, b.number)
                .unwrap()
                .is_none()
        );
        let next = bind(s.conn(), ObjectKind::Plan, "a", "W2", "t").unwrap();
        assert!(
            next.number > b.number,
            "purged global numbers must never be reused"
        );
    }
    #[test]
    fn transaction_rollback_keeps_no_binding() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            bind(&tx, ObjectKind::Plan, "a", "W1", "t").unwrap();
        }
        assert!(
            resolve(s.conn(), ObjectKind::Plan, "a", "W1")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn removed_objects_are_readable_but_new_bindings_are_refused() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let old = bind(s.conn(), ObjectKind::Plan, "a", "W1", "t").unwrap();
        s.conn()
            .execute("UPDATE crews SET archived_at='removed' WHERE id='a'", [])
            .unwrap();
        assert_eq!(
            get(s.conn(), ObjectKind::Plan, old.number).unwrap(),
            Some(old.clone())
        );
        assert_eq!(
            bind(s.conn(), ObjectKind::Plan, "a", "W1", "later").unwrap(),
            old
        );
        assert!(bind(s.conn(), ObjectKind::Plan, "a", "W2", "t").is_err());
        assert!(bind(s.conn(), ObjectKind::Plan, "missing", "W1", "t").is_err());
    }
    #[test]
    fn invalid_source_and_wrong_kind_do_not_resolve() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        for source in ["", " W1", "W1\n"] {
            assert!(bind(s.conn(), ObjectKind::Plan, "a", source, "t").is_err());
        }
        let a = bind(s.conn(), ObjectKind::Plan, "a", "W1", "t").unwrap();
        assert!(
            get(s.conn(), ObjectKind::HumanTask, a.number)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn v4_migration_preserves_source_state_and_takes_a_prewrite_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let s = Store::open(&path).unwrap();
        setup(s.conn());
        let state = r#"{"crew":"a","items":{},"entries":{},"asks":{},"last_item":0,"last_ask":0,"last_seq":0,"clock":0,"stop_streaks":{}}"#;
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('a',?1,'t')",
                [state],
            )
            .unwrap();
        s.conn()
            .execute_batch("DROP TABLE work_stop_checks; DROP TABLE work_events; DROP TABLE object_refs; PRAGMA user_version=4;")
            .unwrap();
        let backups = dir.path().join("backups");
        crate::schema::migrate_guarded(s.conn(), Some(&backups)).unwrap();
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            crate::schema::SCHEMA_VERSION
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT state FROM work_books WHERE crew_id='a'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            state
        );
        let snapshots = std::fs::read_dir(backups)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(snapshots.len(), 1);
        let snapshot = Connection::open(&snapshots[0]).unwrap();
        assert_eq!(
            snapshot
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            snapshot
                .query_row("SELECT state FROM work_books WHERE crew_id='a'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            state
        );
        crate::schema::migrate(s.conn()).unwrap();
        assert_eq!(
            bind(s.conn(), ObjectKind::Plan, "a", "W1", "t")
                .unwrap()
                .number,
            1
        );
    }
    #[test]
    fn refused_migration_keeps_old_version_and_sources() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        s.conn().execute_batch("DROP TABLE work_events; DROP TABLE object_refs; CREATE VIEW object_refs AS SELECT id FROM crews; PRAGMA user_version=4;").unwrap();
        assert!(crate::schema::migrate(s.conn()).is_err());
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM crews", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT type FROM sqlite_master WHERE name='object_refs'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "view"
        );
    }
    #[test]
    fn exhausted_counter_refuses_without_wrapping_or_changing_old_references() {
        let s = Store::open_in_memory().unwrap();
        setup(s.conn());
        let old = bind(s.conn(), ObjectKind::Plan, "a", "W1", "t").unwrap();
        s.conn()
            .execute(
                "UPDATE sqlite_sequence SET seq=?1 WHERE name='object_refs'",
                [i64::MAX],
            )
            .unwrap();
        assert!(bind(s.conn(), ObjectKind::Plan, "a", "W2", "t").is_err());
        assert_eq!(
            get(s.conn(), ObjectKind::Plan, old.number).unwrap(),
            Some(old)
        );
        assert!(
            resolve(s.conn(), ObjectKind::Plan, "a", "W2")
                .unwrap()
                .is_none()
        );
    }
}
