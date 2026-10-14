//! Durable deterministic observation projection. WorkBook and wakes remain authority.
use crate::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, params};
pub const BASELINE_KEY: &str = "work_followup_baseline_required";
pub(crate) const V13_DDL: &str = r#"
CREATE TABLE work_followup_facts(
 id TEXT PRIMARY KEY CHECK(length(id)=64),
 subject_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
 crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 category TEXT NOT NULL CHECK(category IN ('checkpoint','owner_lost','human_wait','dependency_lost')),
 cause TEXT NOT NULL CHECK(json_valid(cause)),
 recipient_snapshot TEXT NOT NULL,
 recipient_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
 stop_floor INTEGER NOT NULL CHECK(stop_floor>=0),
 effect TEXT NOT NULL CHECK(effect IN ('observed','enqueued','delivered','suppressed','unknown')),
 wake_id INTEGER REFERENCES wakes(id) ON DELETE SET NULL,
 original_wake_id INTEGER CHECK(original_wake_id>0),
 observed_at TEXT NOT NULL,
 CHECK(wake_id IS NULL OR wake_id=original_wake_id)
);
CREATE INDEX work_followup_wake ON work_followup_facts(wake_id);
INSERT INTO meta(key,value) VALUES('work_followup_baseline_required','1');
"#;
pub fn available(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='work_followup_facts')",[],|r|r.get(0))?)
}
pub fn baseline_required(c: &Connection) -> Result<bool> {
    match c
        .query_row("SELECT value FROM meta WHERE key=?1", [BASELINE_KEY], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
    {
        Some(s) if s == "1" => Ok(true),
        Some(s) if s == "0" => Ok(false),
        _ => Err(StoreError::Invalid(
            "followup recovery baseline is absent or invalid".into(),
        )),
    }
}
pub fn finish_baseline(c: &Connection) -> Result<()> {
    if c.execute(
        "UPDATE meta SET value='0' WHERE key=?1 AND value='1'",
        [BASELINE_KEY],
    )? != 1
    {
        return Err(StoreError::Invalid(
            "followup baseline transaction lost its authority".into(),
        ));
    }
    Ok(())
}
/// A snapshot can predate unknown/delivered effects. Historical facts must be
/// observed conservatively before any automatic reminder; no exactly-once claim.
pub fn restored(c: &Connection) -> Result<()> {
    if !available(c)? {
        return Ok(());
    }
    c.execute("UPDATE work_followup_facts SET effect=CASE WHEN effect='delivered' THEN effect ELSE 'unknown' END WHERE effect IN ('observed','enqueued')",[])?;
    c.execute("UPDATE wakes SET state='dropped',last_error='followup recovery outcome unknown',settled_at=COALESCE(settled_at,strftime('%Y-%m-%dT%H:%M:%fZ','now')) WHERE state IN ('pending','inflight') AND (source LIKE 'work-check:%' OR id IN (SELECT wake_id FROM work_followup_facts WHERE wake_id IS NOT NULL))",[])?;
    c.execute(
        "INSERT INTO meta(key,value) VALUES(?1,'1') ON CONFLICT(key) DO UPDATE SET value='1'",
        [BASELINE_KEY],
    )?;
    Ok(())
}
/// Called in the owning wake transition transaction. Queue admission is never
/// called delivered, and unacknowledged/requeued input does not become new input.
pub fn wake_transition(c: &Connection, id: i64, before: &str, after: &str) -> Result<()> {
    if !available(c)? {
        return Ok(());
    }
    let effect = if after == "delivered" {
        "delivered"
    } else if before == "inflight" && after != "inflight" {
        "unknown"
    } else if after == "dropped" {
        "suppressed"
    } else {
        return Ok(());
    };
    c.execute("UPDATE work_followup_facts SET effect=?2 WHERE wake_id=?1 AND effect NOT IN ('unknown','suppressed','delivered')",params![id,effect])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn old12() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&c).unwrap();
        c.execute_batch("DROP TABLE work_followup_facts; DELETE FROM meta WHERE key='work_followup_baseline_required'; PRAGMA user_version=12;").unwrap();
        c
    }
    #[test]
    fn formal12_13_atomic_and_historical_baseline_is_not_delivery() {
        let c = old12();
        c.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('preserved','actual source','old','old');").unwrap();
        crate::schema::migrate_guarded(&c, None).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            13
        );
        assert!(baseline_required(&c).unwrap());
        assert_eq!(
            c.query_row("SELECT title FROM crews WHERE id='preserved'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "actual source"
        );
        crate::schema::migrate(&c).unwrap();
        assert!(baseline_required(&c).unwrap());
    }
    #[test]
    fn second_ddl_failure_rolls_back_facts_and_keeps_formal12() {
        let c = old12();
        c.execute_batch("CREATE TABLE foreign_fixture(marker TEXT); CREATE INDEX work_followup_wake ON foreign_fixture(marker);").unwrap();
        assert!(crate::schema::migrate(&c).is_err());
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            12
        );
        assert!(!available(&c).unwrap());
        assert!(crate::meta_get(&c, BASELINE_KEY).unwrap().is_none());
        c.execute_batch("DROP INDEX work_followup_wake;").unwrap();
        crate::schema::migrate(&c).unwrap();
        assert!(available(&c).unwrap());
    }
    #[test]
    fn fresh_database_does_not_suppress_future_new_authoritative_facts() {
        let c = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&c).unwrap();
        assert!(!baseline_required(&c).unwrap());
    }
    #[test]
    fn formal12_protective_backup_failure_and_exact_backup_before13() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        {
            let source = crate::Store::open(&path).unwrap();
            source.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('kept','authoritative old12 source','old','old'); DROP TABLE work_followup_facts; DELETE FROM meta WHERE key='work_followup_baseline_required'; PRAGMA user_version=12;").unwrap();
        }
        std::fs::write(
            dir.path().join("backups"),
            b"protective backup cannot create directory",
        )
        .unwrap();
        assert!(crate::Store::open(&path).is_err());
        let preserved = Connection::open(&path).unwrap();
        assert_eq!(
            preserved
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            12
        );
        assert!(!available(&preserved).unwrap());
        drop(preserved);
        std::fs::remove_file(dir.path().join("backups")).unwrap();
        let upgraded = crate::Store::open(&path).unwrap();
        assert!(baseline_required(upgraded.conn()).unwrap());
        assert_eq!(
            upgraded
                .conn()
                .query_row("SELECT title FROM crews WHERE id='kept'", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "authoritative old12 source"
        );
        let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("crewd-v12-pre-v13-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        let source = Connection::open(&backups[0]).unwrap();
        assert_eq!(
            source
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            12
        );
        assert!(!available(&source).unwrap());
    }
}
