//! Secretary evidence storage. Installation is a separate protected migration;
//! this module never chooses or changes the public database generation.
use crate::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, params};

pub const FORMAT_KEY: &str = "adminSecretary.schemaFormat.v1";
pub const FORMAT: &str = "case-scoped-human-brief.1";
const DATABASE_KEY: &str = "adminSecretary.schemaDatabase.v1";
pub const FACT_BYTES: usize = 65_536;
pub const BODY_BYTES: usize = 8_192;
pub const REFERENCE_LIMIT: usize = 40;
pub const DDL: &str = r#"
CREATE TABLE admin_secretary_sources (
    id TEXT PRIMARY KEY NOT NULL,
    case_id TEXT NOT NULL UNIQUE,
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    source_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    db_id TEXT NOT NULL,
    restore_epoch TEXT NOT NULL,
    judgment_epoch TEXT NOT NULL,
    config_revision TEXT,
    revision TEXT NOT NULL CHECK(length(revision)=64),
    facts_json TEXT NOT NULL CHECK(json_valid(facts_json) AND json_type(facts_json)='object' AND length(CAST(facts_json AS BLOB))<=65536),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0),
    UNIQUE(crew_id,db_id,restore_epoch,revision),
    UNIQUE(id,case_id,revision)
) STRICT;
CREATE TRIGGER admin_secretary_source_scope BEFORE INSERT ON admin_secretary_sources BEGIN
    SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM sessions WHERE id=NEW.source_session_id AND crew_id=NEW.crew_id AND role='captain')
    THEN RAISE(ABORT,'secretary source has no original same-crew captain') END;
END;
CREATE INDEX admin_secretary_sources_crew ON admin_secretary_sources(crew_id,created_at_ms,id);
CREATE TABLE admin_secretary_references (
    source_id TEXT NOT NULL REFERENCES admin_secretary_sources(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('human_wait','case','patrol')),
    reference_id TEXT NOT NULL,
    object_number INTEGER REFERENCES object_refs(number) ON DELETE CASCADE,
    PRIMARY KEY(source_id,kind,reference_id),
    CHECK((kind='human_wait' AND object_number IS NOT NULL AND object_number>0) OR (kind!='human_wait' AND object_number IS NULL))
) STRICT;
CREATE INDEX admin_secretary_reference_object ON admin_secretary_references(object_number,source_id);
CREATE TRIGGER admin_secretary_reference_scope BEFORE INSERT ON admin_secretary_references
WHEN NEW.object_number IS NOT NULL BEGIN
    SELECT CASE WHEN NOT EXISTS(
        SELECT 1 FROM object_refs o JOIN admin_secretary_sources s ON s.id=NEW.source_id
        WHERE o.number=NEW.object_number AND o.kind='human_task' AND o.crew_id=s.crew_id
    ) THEN RAISE(ABORT,'secretary source reference is outside its original crew') END;
END;
CREATE TRIGGER admin_secretary_reference_purge AFTER DELETE ON admin_secretary_references
WHEN OLD.object_number IS NOT NULL AND EXISTS(SELECT 1 FROM admin_secretary_sources WHERE id=OLD.source_id)
BEGIN DELETE FROM admin_secretary_sources WHERE id=OLD.source_id; END;
CREATE TABLE admin_secretary_publications (
    source_id TEXT PRIMARY KEY NOT NULL,
    case_id TEXT NOT NULL UNIQUE,
    source_revision TEXT NOT NULL CHECK(length(source_revision)=64),
    author_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL,
    generation TEXT NOT NULL,
    request_id TEXT NOT NULL UNIQUE,
    command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
    body TEXT NOT NULL CHECK(length(trim(body))>0 AND length(CAST(body AS BLOB))<=8192),
    references_json TEXT NOT NULL CHECK(json_valid(references_json) AND json_type(references_json)='array' AND json_array_length(references_json)<=40 AND length(CAST(references_json AS BLOB))<=16384),
    published_at_ms INTEGER NOT NULL CHECK(published_at_ms>=0),
    revision TEXT NOT NULL CHECK(length(revision)=64),
    FOREIGN KEY(source_id,case_id,source_revision) REFERENCES admin_secretary_sources(id,case_id,revision) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER admin_secretary_publication_scope BEFORE INSERT ON admin_secretary_publications BEGIN
    SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM sessions a JOIN admin_secretary_sources s ON s.id=NEW.source_id WHERE a.id=NEW.author_session_id AND a.crew_id=s.crew_id AND a.role='admin')
    THEN RAISE(ABORT,'secretary author is outside its bound source crew') END;
END;
CREATE TRIGGER admin_secretary_source_immutable BEFORE UPDATE ON admin_secretary_sources
BEGIN SELECT RAISE(ABORT,'immutable secretary source'); END;
CREATE TRIGGER admin_secretary_publication_immutable BEFORE UPDATE ON admin_secretary_publications
BEGIN SELECT RAISE(ABORT,'immutable secretary publication'); END;
CREATE TABLE admin_secretary_observers (
    crew_id TEXT PRIMARY KEY NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    db_id TEXT NOT NULL,
    restore_epoch TEXT NOT NULL,
    judgment_epoch TEXT NOT NULL,
    baseline_revision TEXT NOT NULL CHECK(length(baseline_revision)=64),
    latest_source_id TEXT REFERENCES admin_secretary_sources(id) ON DELETE SET NULL
) STRICT;
CREATE TRIGGER admin_secretary_observer_insert BEFORE INSERT ON admin_secretary_observers
WHEN NEW.latest_source_id IS NOT NULL BEGIN
    SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM admin_secretary_sources WHERE id=NEW.latest_source_id AND crew_id=NEW.crew_id AND db_id=NEW.db_id AND restore_epoch=NEW.restore_epoch AND judgment_epoch=NEW.judgment_epoch)
    THEN RAISE(ABORT,'secretary observer source scope changed') END;
END;
CREATE TRIGGER admin_secretary_observer_update BEFORE UPDATE ON admin_secretary_observers
WHEN NEW.latest_source_id IS NOT NULL BEGIN
    SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM admin_secretary_sources WHERE id=NEW.latest_source_id AND crew_id=NEW.crew_id AND db_id=NEW.db_id AND restore_epoch=NEW.restore_epoch AND judgment_epoch=NEW.judgment_epoch)
    THEN RAISE(ABORT,'secretary observer source scope changed') END;
END;
"#;

fn invalid() -> StoreError {
    StoreError::Invalid("unsupported or invalid secretary evidence".into())
}
fn transaction(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(invalid());
    }
    Ok(())
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn source_id(value: &str) -> bool {
    value
        .strip_prefix("SB")
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        .is_some_and(|id| format!("SB{id}") == value)
}
fn case_id(value: &str) -> bool {
    value.strip_prefix("AC").is_some_and(|v| {
        v.len() == 20
            && v.bytes().all(|b| b.is_ascii_digit())
            && v.parse::<u64>().is_ok_and(|n| n > 0)
    })
}
type Catalog = (String, String, String, Option<String>);
fn objects(c: &Connection) -> Result<Vec<Catalog>> {
    let mut q=c.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE name GLOB 'admin_secretary_*' OR tbl_name GLOB 'admin_secretary_*' ORDER BY type,name,tbl_name")?;
    Ok(
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?,
    )
}
/// Read-only format guard; it does not adopt a table name or repair a partial
/// install. Root assigns the actual public migration before enabling callers.
pub fn validate_schema(c: &Connection) -> Result<()> {
    if crate::meta_get(c, FORMAT_KEY)?.as_deref() != Some(FORMAT)
        || !c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))?
    {
        return Err(invalid());
    }
    let db = crate::meta_get(c, "dbId")?.ok_or_else(invalid)?;
    if db.is_empty() || crate::meta_get(c, DATABASE_KEY)?.as_deref() != Some(db.as_str()) {
        return Err(invalid());
    }
    let reference = Connection::open_in_memory()?;
    reference.execute_batch("CREATE TABLE crews(id TEXT PRIMARY KEY); CREATE TABLE sessions(id TEXT PRIMARY KEY); CREATE TABLE object_refs(number INTEGER PRIMARY KEY,kind TEXT,crew_id TEXT);")?;
    reference.execute_batch(DDL)?;
    if objects(c)? != objects(&reference)? {
        return Err(invalid());
    }
    for name in [
        "admin_secretary_sources",
        "admin_secretary_references",
        "admin_secretary_publications",
        "admin_secretary_observers",
    ] {
        if !c.query_row(
            "SELECT strict=1 AND wr=0 FROM pragma_table_list WHERE schema='main' AND name=?1",
            [name],
            |r| r.get::<_, bool>(0),
        )? || c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check(?1))",
            [name],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid());
        }
    }
    Ok(())
}
pub fn available(c: &Connection) -> Result<bool> {
    let marker = crate::meta_get(c, FORMAT_KEY)?;
    if marker.is_none() && objects(c)?.is_empty() {
        return Ok(false);
    }
    validate_schema(c)?;
    Ok(true)
}
/// Standalone complete migration body. Deliberately not wired to ACTIVE_SCHEMA
/// or an assumed generation number by this independent feature author.
pub fn migrate(c: &Connection) -> Result<()> {
    transaction(c)?;
    if crate::meta_get(c, FORMAT_KEY)?.is_some() || !objects(c)?.is_empty() {
        return Err(invalid());
    }
    let db = crate::meta_get(c, "dbId")?
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)?;
    c.execute_batch(DDL)?;
    crate::meta_set(c, FORMAT_KEY, FORMAT)?;
    crate::meta_set(c, DATABASE_KEY, &db)?;
    validate_schema(c)
}

#[derive(Clone, Debug)]
pub struct Source {
    pub id: String,
    pub case_id: String,
    pub crew_id: String,
    pub source_session_id: String,
    pub db_id: String,
    pub restore_epoch: String,
    pub judgment_epoch: String,
    pub config_revision: Option<String>,
    pub revision: String,
    pub facts_json: String,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub kind: String,
    pub id: String,
    pub object_number: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct Publication {
    pub source_id: String,
    pub case_id: String,
    pub source_revision: String,
    pub author_session_id: String,
    pub attempt_id: String,
    pub generation: String,
    pub request_id: String,
    pub command_digest: String,
    pub body: String,
    pub references_json: String,
    pub published_at_ms: i64,
    pub revision: String,
}
fn source_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Source> {
    Ok(Source {
        id: r.get(0)?,
        case_id: r.get(1)?,
        crew_id: r.get(2)?,
        source_session_id: r.get(3)?,
        db_id: r.get(4)?,
        restore_epoch: r.get(5)?,
        judgment_epoch: r.get(6)?,
        config_revision: r.get(7)?,
        revision: r.get(8)?,
        facts_json: r.get(9)?,
        created_at_ms: r.get(10)?,
    })
}
pub fn source(c: &Connection, id: &str) -> Result<Option<Source>> {
    if !source_id(id) {
        return Err(invalid());
    }
    Ok(c.query_row("SELECT id,case_id,crew_id,source_session_id,db_id,restore_epoch,judgment_epoch,config_revision,revision,facts_json,created_at_ms FROM admin_secretary_sources WHERE id=?1",[id],source_row).optional()?)
}
pub fn source_for_revision(
    c: &Connection,
    crew: &str,
    db: &str,
    epoch: &str,
    revision: &str,
) -> Result<Option<Source>> {
    if !hash(revision) {
        return Err(invalid());
    }
    Ok(c.query_row("SELECT id,case_id,crew_id,source_session_id,db_id,restore_epoch,judgment_epoch,config_revision,revision,facts_json,created_at_ms FROM admin_secretary_sources WHERE crew_id=?1 AND db_id=?2 AND restore_epoch=?3 AND revision=?4",params![crew,db,epoch,revision],source_row).optional()?)
}
pub fn references(c: &Connection, id: &str) -> Result<Vec<Reference>> {
    let mut q=c.prepare("SELECT kind,reference_id,object_number FROM admin_secretary_references WHERE source_id=?1 ORDER BY kind,reference_id LIMIT 41")?;
    let rows = q
        .query_map([id], |r| {
            Ok(Reference {
                kind: r.get(0)?,
                id: r.get(1)?,
                object_number: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > REFERENCE_LIMIT {
        return Err(invalid());
    }
    Ok(rows)
}
pub fn insert_source(c: &Connection, s: &Source, refs: &[Reference]) -> Result<()> {
    transaction(c)?;
    if !source_id(&s.id)
        || !case_id(&s.case_id)
        || !hash(&s.revision)
        || s.facts_json.len() > FACT_BYTES
        || refs.len() > REFERENCE_LIMIT
    {
        return Err(invalid());
    }
    c.execute(
        "INSERT INTO admin_secretary_sources VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            s.id,
            s.case_id,
            s.crew_id,
            s.source_session_id,
            s.db_id,
            s.restore_epoch,
            s.judgment_epoch,
            s.config_revision,
            s.revision,
            s.facts_json,
            s.created_at_ms
        ],
    )?;
    for r in refs {
        c.execute(
            "INSERT INTO admin_secretary_references VALUES(?1,?2,?3,?4)",
            params![s.id, r.kind, r.id, r.object_number],
        )?;
    }
    Ok(())
}
fn publication_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Publication> {
    Ok(Publication {
        source_id: r.get(0)?,
        case_id: r.get(1)?,
        source_revision: r.get(2)?,
        author_session_id: r.get(3)?,
        attempt_id: r.get(4)?,
        generation: r.get(5)?,
        request_id: r.get(6)?,
        command_digest: r.get(7)?,
        body: r.get(8)?,
        references_json: r.get(9)?,
        published_at_ms: r.get(10)?,
        revision: r.get(11)?,
    })
}
const PUBLICATION_COLUMNS: &str = "source_id,case_id,source_revision,author_session_id,attempt_id,generation,request_id,command_digest,body,references_json,published_at_ms,revision";
pub fn publication(c: &Connection, source: &str) -> Result<Option<Publication>> {
    Ok(c.query_row(
        &format!(
            "SELECT {PUBLICATION_COLUMNS} FROM admin_secretary_publications WHERE source_id=?1"
        ),
        [source],
        publication_row,
    )
    .optional()?)
}
pub fn receipt(c: &Connection, request: &str) -> Result<Option<Publication>> {
    Ok(c.query_row(
        &format!(
            "SELECT {PUBLICATION_COLUMNS} FROM admin_secretary_publications WHERE request_id=?1"
        ),
        [request],
        publication_row,
    )
    .optional()?)
}
pub fn insert_publication(c: &Connection, p: &Publication) -> Result<()> {
    transaction(c)?;
    if !source_id(&p.source_id)
        || !case_id(&p.case_id)
        || !hash(&p.source_revision)
        || !hash(&p.command_digest)
        || !hash(&p.revision)
        || p.body.trim().is_empty()
        || p.body.len() > BODY_BYTES
    {
        return Err(invalid());
    }
    c.execute(
        "INSERT INTO admin_secretary_publications VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            p.source_id,
            p.case_id,
            p.source_revision,
            p.author_session_id,
            p.attempt_id,
            p.generation,
            p.request_id,
            p.command_digest,
            p.body,
            p.references_json,
            p.published_at_ms,
            p.revision
        ],
    )?;
    Ok(())
}
#[derive(Clone, Debug)]
pub struct Observer {
    pub crew_id: String,
    pub db_id: String,
    pub restore_epoch: String,
    pub judgment_epoch: String,
    pub baseline_revision: String,
    pub latest_source_id: Option<String>,
}
pub fn observer(c: &Connection, crew: &str) -> Result<Option<Observer>> {
    Ok(c.query_row("SELECT crew_id,db_id,restore_epoch,judgment_epoch,baseline_revision,latest_source_id FROM admin_secretary_observers WHERE crew_id=?1",[crew],|r|Ok(Observer{crew_id:r.get(0)?,db_id:r.get(1)?,restore_epoch:r.get(2)?,judgment_epoch:r.get(3)?,baseline_revision:r.get(4)?,latest_source_id:r.get(5)?})).optional()?)
}
pub fn save_observer(c: &Connection, o: &Observer) -> Result<()> {
    transaction(c)?;
    if !hash(&o.baseline_revision) {
        return Err(invalid());
    }
    c.execute("INSERT INTO admin_secretary_observers VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(crew_id) DO UPDATE SET db_id=excluded.db_id,restore_epoch=excluded.restore_epoch,judgment_epoch=excluded.judgment_epoch,baseline_revision=excluded.baseline_revision,latest_source_id=excluded.latest_source_id",params![o.crew_id,o.db_id,o.restore_epoch,o.judgment_epoch,o.baseline_revision,o.latest_source_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Store,
        object_refs::{self, ObjectKind},
    };
    use crewd_proto::{Crew, Role, Session};

    fn fixture() -> Store {
        let s = Store::open_in_memory().unwrap();
        for crew in ["a", "b"] {
            crate::crews::upsert(
                s.conn(),
                &Crew {
                    id: crew.into(),
                    title: crew.into(),
                    created_at: "2026-11-08T00:00:00Z".into(),
                    updated_at: "2026-11-08T00:00:00Z".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        for (id, crew, role) in [
            ("cap", "a", Role::Captain),
            ("admin", "a", Role::Admin),
            ("other", "b", Role::Admin),
            ("worker", "a", Role::Worker),
        ] {
            crate::sessions::upsert(
                s.conn(),
                &Session {
                    id: id.into(),
                    crew_id: crew.into(),
                    role,
                    executor: "claude".into(),
                    updated_at: "2026-11-08T00:00:00Z".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        s
    }
    fn install(s: &Store) {
        s.conn().execute_batch("BEGIN").unwrap();
        migrate(s.conn()).unwrap();
        s.conn().execute_batch("COMMIT").unwrap();
    }
    fn source_row_for(s: &Store) -> Source {
        Source {
            id: format!("SB{}", uuid::Uuid::new_v4()),
            case_id: "AC00000000000000000001".into(),
            crew_id: "a".into(),
            source_session_id: "cap".into(),
            db_id: crate::meta_get(s.conn(), "dbId").unwrap().unwrap(),
            restore_epoch: "restore".into(),
            judgment_epoch: "judgment".into(),
            config_revision: Some("configuration".into()),
            revision: "a".repeat(64),
            facts_json: "{}".into(),
            created_at_ms: 1,
        }
    }
    fn publication_for(s: &Source) -> Publication {
        Publication {
            source_id: s.id.clone(),
            case_id: s.case_id.clone(),
            source_revision: s.revision.clone(),
            author_session_id: "admin".into(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            generation: "1".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            command_digest: "b".repeat(64),
            body: "保留原始问题和引用。".into(),
            references_json: "[]".into(),
            published_at_ms: 2,
            revision: "c".repeat(64),
        }
    }
    #[test]
    fn migration_is_transactional_explicit_and_checks_exact_catalog() {
        let s = fixture();
        assert!(!available(s.conn()).unwrap());
        assert!(migrate(s.conn()).is_err());
        s.conn().execute_batch("BEGIN").unwrap();
        migrate(s.conn()).unwrap();
        assert!(available(s.conn()).unwrap());
        s.conn().execute_batch("ROLLBACK").unwrap();
        assert!(!available(s.conn()).unwrap());
        install(&s);
        s.conn()
            .execute_batch("DROP TRIGGER admin_secretary_publication_immutable")
            .unwrap();
        assert!(available(s.conn()).is_err());
    }
    #[test]
    fn partial_catalog_and_database_swap_are_not_adopted() {
        let s = fixture();
        s.conn()
            .execute_batch("CREATE TABLE admin_secretary_sources(id TEXT)")
            .unwrap();
        assert!(available(s.conn()).is_err());
        let s = fixture();
        install(&s);
        crate::meta_set(s.conn(), "dbId", "different-database").unwrap();
        assert!(available(s.conn()).is_err());
    }
    #[test]
    fn publication_scope_source_identity_utf8_and_purge_are_enforced() {
        let s = fixture();
        install(&s);
        s.conn().execute_batch("BEGIN").unwrap();
        let row = source_row_for(&s);
        let h = object_refs::bind(
            s.conn(),
            ObjectKind::HumanTask,
            "a",
            "original-human-ask",
            "2026-11-08T00:00:00Z",
        )
        .unwrap();
        let refs = vec![Reference {
            kind: "human_wait".into(),
            id: h.id(),
            object_number: Some(h.number),
        }];
        insert_source(s.conn(), &row, &refs).unwrap();
        let mut p = publication_for(&row);
        p.author_session_id = "other".into();
        assert!(insert_publication(s.conn(), &p).is_err());
        p.author_session_id = "worker".into();
        assert!(insert_publication(s.conn(), &p).is_err());
        p.author_session_id = "admin".into();
        p.case_id = "AC00000000000000000002".into();
        assert!(insert_publication(s.conn(), &p).is_err());
        p.case_id = row.case_id.clone();
        p.body = "界".repeat(BODY_BYTES / 3 + 1);
        assert!(insert_publication(s.conn(), &p).is_err());
        p.body = "有据可查的简报".into();
        insert_publication(s.conn(), &p).unwrap();
        assert!(insert_publication(s.conn(), &p).is_err());
        assert!(
            s.conn()
                .execute("UPDATE admin_secretary_publications SET body='changed'", [])
                .is_err()
        );
        save_observer(
            s.conn(),
            &Observer {
                crew_id: "a".into(),
                db_id: row.db_id.clone(),
                restore_epoch: row.restore_epoch.clone(),
                judgment_epoch: row.judgment_epoch.clone(),
                baseline_revision: row.revision.clone(),
                latest_source_id: Some(row.id.clone()),
            },
        )
        .unwrap();
        s.conn()
            .execute("DELETE FROM object_refs WHERE number=?1", [h.number])
            .unwrap();
        assert!(source(s.conn(), &row.id).unwrap().is_none());
        assert!(receipt(s.conn(), &p.request_id).unwrap().is_none());
        assert!(
            observer(s.conn(), "a")
                .unwrap()
                .unwrap()
                .latest_source_id
                .is_none()
        );
        validate_schema(s.conn()).unwrap();
        s.conn().execute_batch("ROLLBACK").unwrap();
    }
}
