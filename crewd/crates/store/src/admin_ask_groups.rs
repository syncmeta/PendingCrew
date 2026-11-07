//! Advisory metadata installed by the genuine main32 to33 protected migration.
//! Rows carry observations only; original questions and answers remain authoritative.
use crate::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, params};

pub const DDL: &str = r#"
CREATE TABLE admin_ask_group_observations (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=64),
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    first_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
    second_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
    db_id TEXT NOT NULL,
    restore_stamp TEXT NOT NULL,
    config_fingerprint TEXT NOT NULL CHECK(length(config_fingerprint)=64),
    source_fingerprint TEXT NOT NULL CHECK(length(source_fingerprint)=64),
    first_fingerprint TEXT NOT NULL CHECK(length(first_fingerprint)=64),
    second_fingerprint TEXT NOT NULL CHECK(length(second_fingerprint)=64),
    phase TEXT NOT NULL CHECK(phase IN ('prepared','dispatched','suggested','different','unavailable','unknown','stale','invalid')),
    probability REAL CHECK(probability IS NULL OR probability BETWEEN 0 AND 1),
    observed_at_ms INTEGER,
    CHECK(first_number > 0 AND second_number > first_number)
) STRICT;
CREATE INDEX admin_ask_group_pending ON admin_ask_group_observations(phase,id);
CREATE INDEX admin_ask_group_crew ON admin_ask_group_observations(crew_id,id);
CREATE TABLE admin_ask_group_scan (
    crew_id TEXT PRIMARY KEY NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    db_id TEXT NOT NULL,
    restore_stamp TEXT NOT NULL,
    config_fingerprint TEXT NOT NULL,
    floor_number INTEGER NOT NULL CHECK(floor_number>=0),
    anchor_number INTEGER NOT NULL CHECK(anchor_number>=0),
    peer_number INTEGER NOT NULL CHECK(peer_number>=0),
    completed_cut INTEGER NOT NULL CHECK(completed_cut>=0),
    pass_cut INTEGER NOT NULL CHECK(pass_cut>=0),
    generation TEXT NOT NULL,
    pass_generation TEXT NOT NULL
) STRICT;
CREATE TABLE admin_ask_group_scheduler (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    after_crew TEXT REFERENCES crews(id) ON DELETE SET NULL
) STRICT;
CREATE TABLE admin_ask_group_sources (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    db_id TEXT NOT NULL,
    restore_stamp TEXT NOT NULL,
    config_fingerprint TEXT NOT NULL,
    eligibility TEXT NOT NULL CHECK(eligibility IN ('eligible','excluded','unavailable')),
    source_fingerprint TEXT
) STRICT;
CREATE INDEX admin_ask_group_sources_by_crew ON admin_ask_group_sources(crew_id,number);
"#;

/// Distinguishes the supported H-only33 format from the unpublished query33 prototype.
pub const FORMAT_KEY: &str = "adminAskGroups.schemaFormat.v1";
pub const FORMAT: &str = "human-pending-question-groups.1";
const FORMAT_DB_KEY: &str = "adminAskGroups.schemaDatabase.v1";

fn schema_error() -> StoreError {
    StoreError::Invalid("unsupported administrative question-grouping database format".into())
}
type SchemaObject = (String, String, String, Option<String>);
fn schema_objects(c: &Connection) -> Result<Vec<SchemaObject>> {
    let mut q=c.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE name GLOB 'admin_ask_group_*' OR tbl_name GLOB 'admin_ask_group_*' ORDER BY type,name,tbl_name")?;
    Ok(
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    )
}
/// Pure supported-format verifier. Never repairs, initializes or changes PRAGMA.
/// Normative SQLite DDL covers exact columns, checks, STRICT, FK actions and
/// named/automatic indexes. An extra trigger/index on these tables is rejected.
pub fn validate_schema(c: &Connection) -> Result<()> {
    if crate::meta_get(c, FORMAT_KEY)?.as_deref() != Some(FORMAT) {
        return Err(schema_error());
    }
    // Private query33 was never this supported format. Never accept a hybrid
    // by borrowing its marker, table names or a reused numerical version.
    if crate::meta_get(c, "managedViews.nativeEvidenceFormat")?.is_some()
        || c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name GLOB 'work_view_*')",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(schema_error());
    }
    if !c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))? {
        return Err(schema_error());
    }
    let db = crate::meta_get(c, "dbId")?.ok_or_else(schema_error)?;
    if db.is_empty()
        || db.len() > 512
        || db.chars().any(char::is_control)
        || crate::meta_get(c, FORMAT_DB_KEY)?.as_deref() != Some(db.as_str())
    {
        return Err(schema_error());
    }
    crate::personal_drafts::epoch(c)?;
    let reference = Connection::open_in_memory()?;
    reference.execute_batch("CREATE TABLE crews(id TEXT PRIMARY KEY); CREATE TABLE object_refs(number INTEGER PRIMARY KEY);")?;
    reference.execute_batch(DDL)?;
    if schema_objects(c)? != schema_objects(&reference)? {
        return Err(schema_error());
    }
    for table in [
        "admin_ask_group_observations",
        "admin_ask_group_scan",
        "admin_ask_group_scheduler",
        "admin_ask_group_sources",
    ] {
        let strict: bool = c.query_row(
            "SELECT strict=1 AND wr=0 FROM pragma_table_list WHERE schema='main' AND name=?1",
            [table],
            |r| r.get(0),
        )?;
        let broken: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check(?1))",
            [table],
            |r| r.get(0),
        )?;
        if !strict || broken {
            return Err(schema_error());
        }
    }
    let singleton:bool=c.query_row("SELECT count(*)=1 AND COALESCE(min(singleton)=1,0) AND COALESCE(max(singleton)=1,0) FROM admin_ask_group_scheduler",[],|r|r.get(0))?;
    if !singleton {
        return Err(schema_error());
    }
    Ok(())
}

pub fn migrate(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping migration requires a protected transaction".into(),
        ));
    }
    c.execute_batch(DDL)?;
    let db = crate::meta_get(c, "dbId")?
        .ok_or_else(|| StoreError::Invalid("missing Ask grouping database identity".into()))?;
    let epoch = crate::personal_drafts::epoch(c)?;
    let scope = crewd_proto::managed_objects::Scope {
        db_id: db,
        restore_epoch: epoch,
    };
    let crews = {
        let mut q = c.prepare(
            "SELECT DISTINCT crew_id FROM object_refs WHERE kind='human_task' ORDER BY crew_id",
        )?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for crew in crews {
        ensure_cursor(c, &crew, &scope, "")?;
    }
    c.execute(
        "INSERT INTO admin_ask_group_scheduler(singleton,after_crew) VALUES(1,NULL)",
        [],
    )?;
    c.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2)",
        params![FORMAT_KEY, FORMAT],
    )?;
    c.execute(
        "INSERT INTO meta(key,value) SELECT ?1,value FROM meta WHERE key='dbId'",
        [FORMAT_DB_KEY],
    )?;
    validate_schema(c)
}

pub fn available(c: &Connection) -> Result<bool> {
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 33 {
        return Ok(false);
    }
    let present=c.query_row("SELECT count(*)=4 FROM sqlite_master WHERE type='table' AND name IN ('admin_ask_group_observations','admin_ask_group_scan','admin_ask_group_sources','admin_ask_group_scheduler')", [], |r|r.get::<_,bool>(0))?;
    if !present {
        return Ok(false);
    }
    if crate::meta_get(c, FORMAT_KEY)?.as_deref() != Some(FORMAT) {
        return Ok(false);
    }
    let db = crate::meta_get(c, "dbId")?;
    if db.as_deref().is_none_or(str::is_empty) || crate::meta_get(c, FORMAT_DB_KEY)? != db {
        return Ok(false);
    }
    crate::personal_drafts::epoch(c)?;
    Ok(c.query_row(
        "SELECT count(*)=1 FROM admin_ask_group_scheduler WHERE singleton=1",
        [],
        |r| r.get(0),
    )?)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub id: String,
    pub crew: String,
    pub first: i64,
    pub second: i64,
    pub db: String,
    pub epoch: String,
    pub config: String,
    pub fingerprint: String,
    pub first_fingerprint: String,
    pub second_fingerprint: String,
    pub phase: String,
    pub probability: Option<f64>,
    pub observed_at: Option<i64>,
}

fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Observation> {
    Ok(Observation {
        id: r.get(0)?,
        crew: r.get(1)?,
        first: r.get(2)?,
        second: r.get(3)?,
        db: r.get(4)?,
        epoch: r.get(5)?,
        config: r.get(6)?,
        fingerprint: r.get(7)?,
        phase: r.get(8)?,
        probability: r.get(9)?,
        observed_at: r.get(10)?,
        first_fingerprint: r.get(11)?,
        second_fingerprint: r.get(12)?,
    })
}
const COLUMNS: &str = "id,crew_id,first_number,second_number,db_id,restore_stamp,config_fingerprint,source_fingerprint,phase,probability,observed_at_ms,first_fingerprint,second_fingerprint";

pub fn get(c: &Connection, id: &str) -> Result<Option<Observation>> {
    Ok(c.query_row(
        &format!("SELECT {COLUMNS} FROM admin_ask_group_observations WHERE id=?1"),
        [id],
        read,
    )
    .optional()?)
}

pub fn prepare(c: &Connection, row: &Observation) -> Result<bool> {
    if c.is_autocommit()
        || row.phase != "prepared"
        || row.probability.is_some()
        || row.observed_at.is_some()
    {
        return Err(StoreError::Invalid(
            "invalid prepared Ask grouping observation".into(),
        ));
    }
    let n=c.execute("INSERT INTO admin_ask_group_observations(id,crew_id,first_number,second_number,db_id,restore_stamp,config_fingerprint,source_fingerprint,phase,first_fingerprint,second_fingerprint) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'prepared',?9,?10) ON CONFLICT(id) DO NOTHING",params![row.id,row.crew,row.first,row.second,row.db,row.epoch,row.config,row.fingerprint,row.first_fingerprint,row.second_fingerprint])?;
    Ok(n == 1)
}

pub fn transition(
    c: &Connection,
    id: &str,
    expected: &str,
    phase: &str,
    probability: Option<f64>,
    at: Option<i64>,
) -> Result<bool> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping transition requires a transaction".into(),
        ));
    }
    Ok(c.execute("UPDATE admin_ask_group_observations SET phase=?3,probability=?4,observed_at_ms=?5 WHERE id=?1 AND phase=?2",params![id,expected,phase,probability,at])?==1)
}

pub fn pending(c: &Connection, limit: u16) -> Result<Vec<Observation>> {
    let mut q=c.prepare(&format!("SELECT {COLUMNS} FROM admin_ask_group_observations WHERE phase='prepared' ORDER BY rowid LIMIT ?1"))?;
    Ok(q.query_map([i64::from(limit.clamp(1, 64))], read)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Recovery never turns a dispatched external request back into executable work.
pub fn recover(c: &Connection, limit: u16) -> Result<usize> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping recovery requires a transaction".into(),
        ));
    }
    Ok(c.execute("UPDATE admin_ask_group_observations SET phase='unknown' WHERE id IN (SELECT id FROM admin_ask_group_observations WHERE phase='dispatched' ORDER BY rowid LIMIT ?1)",[i64::from(limit.clamp(1,64))])?)
}

pub fn page(
    c: &Connection,
    crew: &str,
    after: Option<&str>,
    limit: u16,
) -> Result<(Vec<Observation>, Option<String>)> {
    let limit = limit.clamp(1, 20);
    let mut q=c.prepare(&format!("SELECT {COLUMNS} FROM admin_ask_group_observations WHERE crew_id=?1 AND id>?2 ORDER BY id LIMIT ?3"))?;
    let mut rows = q
        .query_map(
            params![crew, after.unwrap_or(""), i64::from(limit) + 1],
            read,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next = if rows.len() > usize::from(limit) {
        rows.pop();
        rows.last().map(|r| r.id.clone())
    } else {
        None
    };
    Ok((rows, next))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    pub crew: String,
    pub db: String,
    pub epoch: String,
    pub config: String,
    pub floor: i64,
    pub anchor: i64,
    pub peer: i64,
    pub completed_cut: i64,
    pub pass_cut: i64,
    pub generation: String,
    pub pass_generation: String,
}
fn read_cursor(r: &rusqlite::Row<'_>) -> rusqlite::Result<Cursor> {
    Ok(Cursor {
        crew: r.get(0)?,
        db: r.get(1)?,
        epoch: r.get(2)?,
        config: r.get(3)?,
        floor: r.get(4)?,
        anchor: r.get(5)?,
        peer: r.get(6)?,
        completed_cut: r.get(7)?,
        pass_cut: r.get(8)?,
        generation: r.get(9)?,
        pass_generation: r.get(10)?,
    })
}
const SCAN_COLUMNS: &str = "crew_id,db_id,restore_stamp,config_fingerprint,floor_number,anchor_number,peer_number,completed_cut,pass_cut,generation,pass_generation";
pub fn cursor(c: &Connection, crew: &str) -> Result<Option<Cursor>> {
    let result = c
        .query_row(
            &format!("SELECT {SCAN_COLUMNS} FROM admin_ask_group_scan WHERE crew_id=?1"),
            [crew],
            read_cursor,
        )
        .optional()?;
    if result.as_ref().is_some_and(|v| {
        v.crew != crew
            || v.db.is_empty()
            || !crewd_proto::valid_remote_uuid(&v.epoch)
            || !crewd_proto::valid_remote_uuid(&v.generation)
            || !crewd_proto::valid_remote_uuid(&v.pass_generation)
            || v.floor < 0
            || v.anchor < v.floor
            || v.peer < 0
            || v.completed_cut < v.floor
            || v.pass_cut < v.anchor
    }) {
        return Err(StoreError::Invalid(
            "invalid Ask grouping scan metadata".into(),
        ));
    }
    Ok(result)
}
pub fn cut(c: &Connection, crew: &str) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COALESCE(MAX(number),0) FROM object_refs WHERE kind='human_task' AND crew_id=?1",
        [crew],
        |r| r.get(0),
    )?)
}
/// Enroll one actual published H's crew without decoding optional old projection.
pub fn enroll_cursor(
    c: &Connection,
    crew: &str,
    scope: &crewd_proto::managed_objects::Scope,
    config: &str,
) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping enrollment requires a transaction".into(),
        ));
    }
    let generation = uuid::Uuid::new_v4().to_string();
    c.execute("INSERT INTO admin_ask_group_scan(crew_id,db_id,restore_stamp,config_fingerprint,floor_number,anchor_number,peer_number,completed_cut,pass_cut,generation,pass_generation) VALUES(?1,?2,?3,?4,0,0,0,0,?5,?6,?6) ON CONFLICT(crew_id) DO NOTHING",params![crew,scope.db_id,scope.restore_epoch,config,cut(c,crew)?,generation])?;
    Ok(())
}
pub fn ensure_cursor(
    c: &Connection,
    crew: &str,
    scope: &crewd_proto::managed_objects::Scope,
    config: &str,
) -> Result<Cursor> {
    enroll_cursor(c, crew, scope, config)?;
    cursor(c, crew)?.ok_or_else(|| StoreError::Invalid("missing Ask grouping scope".into()))
}
pub fn pass_cut(c: &Connection, crew: &str) -> Result<Option<i64>> {
    Ok(c.query_row(
        "SELECT pass_cut FROM admin_ask_group_scan WHERE crew_id=?1",
        [crew],
        |r| r.get(0),
    )
    .optional()?)
}

pub fn save_cursor(c: &Connection, row: &Cursor) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping scan requires a transaction".into(),
        ));
    }
    c.execute("INSERT INTO admin_ask_group_scan(crew_id,db_id,restore_stamp,config_fingerprint,floor_number,anchor_number,peer_number,completed_cut,pass_cut,generation,pass_generation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(crew_id) DO UPDATE SET db_id=excluded.db_id,restore_stamp=excluded.restore_stamp,config_fingerprint=excluded.config_fingerprint,floor_number=excluded.floor_number,anchor_number=excluded.anchor_number,peer_number=excluded.peer_number,completed_cut=excluded.completed_cut,pass_cut=excluded.pass_cut,generation=excluded.generation,pass_generation=excluded.pass_generation WHERE admin_ask_group_scan.db_id IS NOT excluded.db_id OR admin_ask_group_scan.restore_stamp IS NOT excluded.restore_stamp OR admin_ask_group_scan.config_fingerprint IS NOT excluded.config_fingerprint OR admin_ask_group_scan.floor_number IS NOT excluded.floor_number OR admin_ask_group_scan.anchor_number IS NOT excluded.anchor_number OR admin_ask_group_scan.peer_number IS NOT excluded.peer_number OR admin_ask_group_scan.completed_cut IS NOT excluded.completed_cut OR admin_ask_group_scan.pass_cut IS NOT excluded.pass_cut OR admin_ask_group_scan.generation IS NOT excluded.generation OR admin_ask_group_scan.pass_generation IS NOT excluded.pass_generation",params![row.crew,row.db,row.epoch,row.config,row.floor,row.anchor,row.peer,row.completed_cut,row.pass_cut,row.generation,row.pass_generation])?;
    Ok(())
}
/// Only actual source changes use this; advice and ordinary replay do not.
pub fn invalidate_coverage(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping invalidation requires a transaction".into(),
        ));
    }
    c.execute(
        "UPDATE admin_ask_group_scan SET generation=?1,completed_cut=floor_number",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    Ok(())
}
pub fn next_crew(c: &Connection) -> Result<Option<String>> {
    let after: Option<String> = c.query_row(
        "SELECT after_crew FROM admin_ask_group_scheduler WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let next=c.query_row("SELECT s.crew_id FROM admin_ask_group_scan s JOIN crews c ON c.id=s.crew_id WHERE c.archived_at IS NULL AND s.crew_id>?1 ORDER BY s.crew_id LIMIT 1",[after.as_deref().unwrap_or("")],|r|r.get::<_,String>(0)).optional()?;
    let next=match next{Some(v)=>Some(v),None=>c.query_row("SELECT s.crew_id FROM admin_ask_group_scan s JOIN crews c ON c.id=s.crew_id WHERE c.archived_at IS NULL ORDER BY s.crew_id LIMIT 1",[],|r|r.get(0)).optional()?};
    if let Some(crew) = &next {
        c.execute("UPDATE admin_ask_group_scheduler SET after_crew=?1 WHERE singleton=1 AND after_crew IS NOT ?1",[crew])?;
    }
    Ok(next)
}

/// A bounded restore pass, independent of current feature opt-in.
pub fn stale_cursors(
    c: &Connection,
    scope: &crewd_proto::managed_objects::Scope,
    limit: u16,
) -> Result<Vec<Cursor>> {
    let mut q=c.prepare(&format!("SELECT {SCAN_COLUMNS} FROM admin_ask_group_scan WHERE db_id<>?1 OR restore_stamp<>?2 ORDER BY crew_id LIMIT ?3"))?;
    Ok(q.query_map(
        params![
            scope.db_id,
            scope.restore_epoch,
            i64::from(limit.clamp(1, 64))
        ],
        read_cursor,
    )?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn note_source(
    c: &Connection,
    number: i64,
    crew: &str,
    scope: &crewd_proto::managed_objects::Scope,
    config: &str,
    eligibility: &str,
    fingerprint: Option<&str>,
) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Ask grouping source proof requires a transaction".into(),
        ));
    }
    c.execute("INSERT INTO admin_ask_group_sources(number,crew_id,db_id,restore_stamp,config_fingerprint,eligibility,source_fingerprint) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(number) DO UPDATE SET crew_id=excluded.crew_id,db_id=excluded.db_id,restore_stamp=excluded.restore_stamp,config_fingerprint=excluded.config_fingerprint,eligibility=excluded.eligibility,source_fingerprint=excluded.source_fingerprint WHERE admin_ask_group_sources.crew_id IS NOT excluded.crew_id OR admin_ask_group_sources.db_id IS NOT excluded.db_id OR admin_ask_group_sources.restore_stamp IS NOT excluded.restore_stamp OR admin_ask_group_sources.config_fingerprint IS NOT excluded.config_fingerprint OR admin_ask_group_sources.eligibility IS NOT excluded.eligibility OR admin_ask_group_sources.source_fingerprint IS NOT excluded.source_fingerprint",params![number,crew,scope.db_id,scope.restore_epoch,config,eligibility,fingerprint])?;
    Ok(())
}

pub fn has_gaps(
    c: &Connection,
    crew: &str,
    scope: &crewd_proto::managed_objects::Scope,
    config: &str,
) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM admin_ask_group_sources WHERE crew_id=?1 AND db_id=?2 AND restore_stamp=?3 AND config_fingerprint=?4 AND eligibility='unavailable')",params![crew,scope.db_id,scope.restore_epoch,config],|r|r.get(0))?)
}

pub fn unresolved(
    c: &Connection,
    crew: &str,
    scope: &crewd_proto::managed_objects::Scope,
    config: &str,
) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM admin_ask_group_observations o JOIN admin_ask_group_sources a ON a.number=o.first_number JOIN admin_ask_group_sources b ON b.number=o.second_number WHERE o.crew_id=?1 AND o.db_id=?2 AND o.restore_stamp=?3 AND o.config_fingerprint=?4 AND o.phase IN ('prepared','dispatched','unknown','unavailable','stale','invalid') AND a.eligibility='eligible' AND b.eligibility='eligible' AND a.db_id=o.db_id AND b.db_id=o.db_id AND a.restore_stamp=o.restore_stamp AND b.restore_stamp=o.restore_stamp AND a.config_fingerprint=o.config_fingerprint AND b.config_fingerprint=o.config_fingerprint AND a.source_fingerprint=o.first_fingerprint AND b.source_fingerprint=o.second_fingerprint)",params![crew,scope.db_id,scope.restore_epoch,config],|r|r.get(0))?)
}
