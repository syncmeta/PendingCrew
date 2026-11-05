//! Disposable, retained search projections. Never an authority for content or ACL.
use crate::{Result, StoreError};
use crewd_proto::{
    managed_tables_v2::{EdgeView, TargetRef},
    organization_search::{Kind, MatchOn, Status},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const CLOSED_PER_SOURCE: i64 = 64;
pub const CLOSED_TOTAL: i64 = 100_000;
pub const HISTORY_BYTES: i64 = 128 * 1024 * 1024;
pub const BATCH: u16 = 64;
pub const DDL: &str = r#"
CREATE TABLE organization_search_state(
 id INTEGER PRIMARY KEY CHECK(id=1), generation TEXT NOT NULL, clock INTEGER NOT NULL CHECK(typeof(clock)='integer' AND clock>=0),
 min_cut INTEGER NOT NULL CHECK(typeof(min_cut)='integer' AND min_cut>=0), closed_count INTEGER NOT NULL DEFAULT 0 CHECK(typeof(closed_count)='integer' AND closed_count>=0), phase INTEGER NOT NULL DEFAULT 0 CHECK(phase BETWEEN 0 AND 8), after_key TEXT NOT NULL DEFAULT '', build_cut INTEGER DEFAULT 0, history_bytes INTEGER NOT NULL DEFAULT 0 CHECK(typeof(history_bytes)='integer' AND history_bytes>=0)
);
INSERT INTO organization_search_state(id,generation,clock,min_cut) VALUES(1,lower(hex(randomblob(16))),0,0);
CREATE TABLE organization_search_times(source TEXT PRIMARY KEY,changed_at TEXT,revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0)) WITHOUT ROWID;
CREATE TABLE organization_search_dirty(source TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),span TEXT) WITHOUT ROWID;
CREATE TABLE organization_search_sources(
 source TEXT PRIMARY KEY, target TEXT NOT NULL CHECK(json_valid(target)),
 object_number INTEGER REFERENCES object_refs(number) ON DELETE CASCADE,
 message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
 artifact_reference TEXT REFERENCES managed_artifacts(reference) ON DELETE CASCADE,
 crew_id TEXT REFERENCES crews(id) ON DELETE CASCADE,
 session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
 channel_id TEXT REFERENCES channels(id) ON DELETE CASCADE,
 CHECK((object_number IS NOT NULL)+(message_id IS NOT NULL)+(artifact_reference IS NOT NULL)+(crew_id IS NOT NULL)+(session_id IS NOT NULL)+(channel_id IS NOT NULL)<=1)
) WITHOUT ROWID;
CREATE UNIQUE INDEX organization_search_target ON organization_search_sources(target);
CREATE TRIGGER organization_search_source_delete BEFORE DELETE ON organization_search_sources BEGIN DELETE FROM organization_search_edges WHERE target=OLD.target; DELETE FROM organization_search_dirty WHERE source=OLD.source; END;
CREATE TABLE organization_search_versions(
 id INTEGER PRIMARY KEY AUTOINCREMENT, source TEXT NOT NULL REFERENCES organization_search_sources(source) ON DELETE CASCADE,
 valid_from INTEGER NOT NULL CHECK(valid_from>0), valid_to INTEGER CHECK(valid_to IS NULL OR valid_to>valid_from),
 kind TEXT, owner_crew TEXT, table_id TEXT, channel_id TEXT, changed_at TEXT, status_rank INTEGER NOT NULL CHECK(status_rank BETWEEN 0 AND 2), system_default INTEGER NOT NULL CHECK(system_default IN(0,1)),
 fingerprint TEXT NOT NULL, document TEXT NOT NULL CHECK(json_valid(document)), UNIQUE(source,valid_from)
);
CREATE TRIGGER organization_search_version_close AFTER UPDATE OF valid_to ON organization_search_versions WHEN OLD.valid_to IS NULL AND NEW.valid_to IS NOT NULL BEGIN UPDATE organization_search_state SET closed_count=closed_count+1 WHERE id=1; END;
CREATE TRIGGER organization_search_version_delete AFTER DELETE ON organization_search_versions WHEN OLD.valid_to IS NOT NULL BEGIN UPDATE organization_search_state SET closed_count=closed_count-1 WHERE id=1; END;
CREATE TABLE organization_search_gc(version_id INTEGER PRIMARY KEY REFERENCES organization_search_versions(id) ON DELETE CASCADE,valid_to INTEGER NOT NULL CHECK(valid_to>0),span TEXT);
CREATE TRIGGER organization_search_history_close AFTER UPDATE OF valid_to ON organization_search_versions WHEN OLD.valid_to IS NULL AND NEW.valid_to IS NOT NULL BEGIN UPDATE organization_search_state SET history_bytes=history_bytes+length(CAST(NEW.document AS BLOB))+COALESCE((SELECT sum(length(CAST(body AS BLOB))) FROM organization_search_slots WHERE version_id=NEW.id),0) WHERE id=1; END;
CREATE TRIGGER organization_search_history_header_delete BEFORE DELETE ON organization_search_versions WHEN OLD.valid_to IS NOT NULL BEGIN UPDATE organization_search_state SET history_bytes=history_bytes-length(CAST(OLD.document AS BLOB))-COALESCE((SELECT sum(length(CAST(body AS BLOB))) FROM organization_search_slots WHERE version_id=OLD.id),0) WHERE id=1; END;
CREATE UNIQUE INDEX organization_search_current ON organization_search_versions(source) WHERE valid_to IS NULL;
CREATE INDEX organization_search_order ON organization_search_versions(status_rank,changed_at DESC,valid_from DESC,source);
CREATE INDEX organization_search_owner ON organization_search_versions(owner_crew,source) WHERE valid_to IS NULL;
CREATE INDEX organization_search_closed ON organization_search_versions(valid_to,id) WHERE valid_to IS NOT NULL;
CREATE TABLE organization_search_slots(id INTEGER PRIMARY KEY AUTOINCREMENT,version_id INTEGER NOT NULL REFERENCES organization_search_versions(id) ON DELETE CASCADE,field_id TEXT NOT NULL,slot_kind TEXT NOT NULL,body TEXT NOT NULL,UNIQUE(version_id,field_id,slot_kind));
CREATE VIRTUAL TABLE organization_search_fts USING fts5(body,content='organization_search_slots',content_rowid='id',tokenize='trigram');
CREATE TRIGGER organization_search_slot_insert AFTER INSERT ON organization_search_slots BEGIN INSERT INTO organization_search_fts(rowid,body) VALUES(NEW.id,NEW.body); END;
CREATE TRIGGER organization_search_slot_delete AFTER DELETE ON organization_search_slots BEGIN INSERT INTO organization_search_fts(organization_search_fts,rowid,body) VALUES('delete',OLD.id,OLD.body); END;
CREATE TRIGGER organization_search_slot_update AFTER UPDATE ON organization_search_slots BEGIN INSERT INTO organization_search_fts(organization_search_fts,rowid,body) VALUES('delete',OLD.id,OLD.body); INSERT INTO organization_search_fts(rowid,body) VALUES(NEW.id,NEW.body); END;
CREATE TRIGGER organization_search_history_slot_delete AFTER DELETE ON organization_search_slots WHEN EXISTS(SELECT 1 FROM organization_search_versions WHERE id=OLD.version_id AND valid_to IS NOT NULL) BEGIN UPDATE organization_search_state SET history_bytes=history_bytes-length(CAST(OLD.body AS BLOB)) WHERE id=1; END;
CREATE TABLE organization_search_aliases(id INTEGER PRIMARY KEY AUTOINCREMENT,table_number INTEGER NOT NULL REFERENCES managed_tables(number) ON DELETE CASCADE,event_id TEXT NOT NULL REFERENCES managed_table_schema_events(event_id) ON DELETE CASCADE,field_id TEXT NOT NULL,label TEXT NOT NULL,visible_from INTEGER NOT NULL CHECK(typeof(visible_from)='integer' AND visible_from>=0),UNIQUE(event_id,field_id));
CREATE INDEX organization_search_alias_target ON organization_search_aliases(table_number,id);
CREATE VIRTUAL TABLE organization_search_alias_fts USING fts5(label,content='organization_search_aliases',content_rowid='id',tokenize='trigram');
CREATE TRIGGER organization_search_alias_insert AFTER INSERT ON organization_search_aliases BEGIN INSERT INTO organization_search_alias_fts(rowid,label) VALUES(NEW.id,NEW.label); END;
CREATE TRIGGER organization_search_alias_delete AFTER DELETE ON organization_search_aliases BEGIN INSERT INTO organization_search_alias_fts(organization_search_alias_fts,rowid,label) VALUES('delete',OLD.id,OLD.label); END;
CREATE TABLE organization_search_fanout(crew_id TEXT PRIMARY KEY REFERENCES crews(id) ON DELETE CASCADE,after_key TEXT NOT NULL DEFAULT '',revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0)) WITHOUT ROWID;
CREATE TABLE organization_search_graph_jobs(source TEXT PRIMARY KEY REFERENCES organization_search_sources(source) ON DELETE CASCADE,fingerprint TEXT NOT NULL,after_key TEXT,seen_generation INTEGER NOT NULL CHECK(seen_generation>=0),span TEXT) WITHOUT ROWID;
CREATE TABLE organization_search_edges(
 id INTEGER PRIMARY KEY AUTOINCREMENT, edge_key TEXT NOT NULL, source TEXT NOT NULL REFERENCES organization_search_sources(source) ON DELETE CASCADE, owner_crew TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 target TEXT NOT NULL CHECK(json_valid(target)),target_source TEXT NOT NULL REFERENCES organization_search_sources(source) ON DELETE CASCADE, valid_from INTEGER NOT NULL CHECK(valid_from>0),valid_to INTEGER CHECK(valid_to IS NULL OR valid_to>valid_from),
 body TEXT NOT NULL CHECK(json_valid(body)), seen_stamp INTEGER NOT NULL, UNIQUE(edge_key,valid_from)
);
CREATE UNIQUE INDEX organization_search_edge_current ON organization_search_edges(edge_key) WHERE valid_to IS NULL;
CREATE INDEX organization_search_edge_source ON organization_search_edges(source,valid_from,valid_to);
CREATE INDEX organization_search_edge_target ON organization_search_edges(target,valid_from,valid_to);
"#;

fn bad() -> StoreError {
    StoreError::Invalid("invalid organization search projection".into())
}
fn transaction(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        Err(bad())
    } else {
        Ok(())
    }
}
pub fn enabled(c: &Connection) -> Result<bool> {
    let yes:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='organization_search_state')",[],|r|r.get(0))?;
    if !yes && c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? >= 31 {
        return Err(bad());
    }
    Ok(yes)
}
pub fn key(target: &TargetRef) -> Result<String> {
    Ok(serde_json::to_string(target)?)
}
pub fn source_object(n: i64) -> String {
    format!("o:{n:020}")
}
pub fn source_message(id: &str) -> String {
    format!("m:{id}")
}
pub fn source_artifact(reference: &str) -> String {
    format!("a:{reference}")
}

/// Install at migration time. Triggers cover real SQL writes, including legacy V1.
pub fn install(c: &Connection) -> Result<()> {
    crate::search_context::ensure_registered(c)?;
    transaction(c)?;
    c.execute_batch(DDL)?;
    for (table, column, prefix, numeric) in [
        ("object_refs", "number", "o:", true),
        ("managed_tables", "number", "o:", true),
        ("managed_table_attributes", "number", "o:", true),
        ("managed_rows", "number", "o:", true),
        ("managed_decisions", "number", "o:", true),
        ("managed_risks", "number", "o:", true),
        ("managed_lessons", "number", "o:", true),
        ("messages", "id", "m:", false),
        ("managed_artifacts", "reference", "a:", false),
        ("crews", "id", "c:", false),
        ("sessions", "id", "s:", false),
        ("channels", "id", "h:", false),
    ] {
        let key = if numeric {
            format!("'{prefix}'||printf('%020d',NEW.{column})")
        } else {
            format!("'{prefix}'||NEW.{column}")
        };
        for action in ["INSERT", "UPDATE"] {
            let condition = if action == "UPDATE" {
                format!("WHEN {}", changed_columns(c, table)?)
            } else {
                String::new()
            };
            c.execute_batch(&format!("CREATE TRIGGER organization_search_{table}_{} AFTER {action} ON {table} {condition} BEGIN {} END;",action.to_ascii_lowercase(),queue_sql(&key,action=="UPDATE")))?;
        }
    }
    c.execute_batch("CREATE TRIGGER organization_search_schema_alias AFTER INSERT ON managed_table_schema_events BEGIN INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT NEW.table_number,NEW.event_id,'',json_extract(NEW.before_json,'$.title'),(SELECT clock+1 FROM organization_search_state WHERE id=1) WHERE json_extract(NEW.before_json,'$.title')!=json_extract(NEW.after_json,'$.title'); INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT NEW.table_number,NEW.event_id,'@purpose',json_extract(NEW.before_json,'$.purpose'),(SELECT clock+1 FROM organization_search_state WHERE id=1) WHERE json_extract(NEW.before_json,'$.purpose') IS NOT NULL AND json_extract(NEW.before_json,'$.purpose') IS NOT json_extract(NEW.after_json,'$.purpose'); INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT NEW.table_number,NEW.event_id,json_extract(b.value,'$.id'),json_extract(b.value,'$.name'),(SELECT clock+1 FROM organization_search_state WHERE id=1) FROM json_each(NEW.before_json,'$.fields') b JOIN json_each(NEW.after_json,'$.fields') a ON json_extract(a.value,'$.id')=json_extract(b.value,'$.id') WHERE json_extract(a.value,'$.name')!=json_extract(b.value,'$.name'); END;")?;
    // Raw requirement edits must not wait for a WorkBook event.
    let todo_key = "'o:'||(SELECT printf('%020d',number) FROM object_refs WHERE kind='requirement' AND source_id='todo:'||NEW.id)";
    c.execute_batch(&format!("CREATE TRIGGER organization_search_todo_update AFTER UPDATE ON todos WHEN EXISTS(SELECT 1 FROM object_refs WHERE kind='requirement' AND source_id='todo:'||NEW.id) BEGIN {} END;",queue_sql(todo_key,true)))?;
    c.execute_batch("CREATE TRIGGER organization_search_todo_attribute AFTER INSERT ON todo_attribute_events BEGIN UPDATE organization_search_state SET build_cut=COALESCE(build_cut,clock) WHERE id=1; INSERT INTO organization_search_dirty SELECT 'o:'||printf('%020d',number),1,pcw_search_span() FROM object_refs WHERE(kind='requirement' AND '#'||number=NEW.object_id) OR(kind='human_task' AND 'H'||number=NEW.object_id) ON CONFLICT(source) DO UPDATE SET revision=revision+1,span=excluded.span; INSERT INTO organization_search_times SELECT 'o:'||printf('%020d',number),NEW.at,1 FROM object_refs WHERE(kind='requirement' AND '#'||number=NEW.object_id) OR(kind='human_task' AND 'H'||number=NEW.object_id) ON CONFLICT(source) DO UPDATE SET changed_at=excluded.changed_at,revision=revision+1; END;")?;

    for table in [
        "managed_links",
        "managed_artifact_edges",
        "managed_message_sources",
    ] {
        // Each table has a precise owning subject; DELETE uses OLD, never a scan.
        let col = if table == "managed_links" {
            "source_number"
        } else if table == "managed_artifact_edges" {
            "target_number"
        } else {
            "number"
        };
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let row = if action == "DELETE" { "OLD" } else { "NEW" };
            let key = format!("'o:'||printf('%020d',{row}.{col})");
            c.execute_batch(&format!("CREATE TRIGGER organization_search_{table}_{} AFTER {action} ON {table} WHEN {row}.{col} IS NOT NULL BEGIN {} END;",action.to_ascii_lowercase(),queue_sql(&key,true)))?;
        }
    }
    c.execute_batch("CREATE TRIGGER organization_search_crew_archive AFTER UPDATE OF archived_at ON crews WHEN OLD.archived_at IS NOT NEW.archived_at BEGIN INSERT INTO organization_search_fanout VALUES(NEW.id,'',1) ON CONFLICT(crew_id) DO UPDATE SET after_key='',revision=revision+1; END;")?;
    for (table, column, prefix) in [
        ("object_refs", "number", "o:"),
        ("messages", "id", "m:"),
        ("managed_artifacts", "reference", "a:"),
        ("crews", "id", "c:"),
        ("sessions", "id", "s:"),
        ("channels", "id", "h:"),
    ] {
        let key = if table == "object_refs" {
            format!("'{prefix}'||printf('%020d',OLD.{column})")
        } else {
            format!("'{prefix}'||OLD.{column}")
        };
        c.execute_batch(&format!("CREATE TRIGGER organization_search_{table}_delete AFTER DELETE ON {table} BEGIN DELETE FROM organization_search_dirty WHERE source={key}; DELETE FROM organization_search_times WHERE source={key}; END;"))?;
    }
    for action in ["INSERT", "UPDATE", "DELETE"] {
        let row = if action == "DELETE" { "OLD" } else { "NEW" };
        c.execute_batch(&format!("CREATE TRIGGER organization_search_artifact_source_{} AFTER {action} ON managed_links WHEN {row}.source_artifact IS NOT NULL BEGIN {} END;",action.to_ascii_lowercase(),queue_sql(&format!("'a:'||{row}.source_artifact"),true)))?;
    }
    Ok(())
}

pub fn mark(c: &Connection, source: &str) -> Result<()> {
    if !enabled(c)? {
        return Ok(());
    }
    transaction(c)?;
    c.execute(
        "UPDATE organization_search_state SET build_cut=COALESCE(build_cut,clock) WHERE id=1",
        [],
    )?;
    c.execute("INSERT INTO organization_search_dirty VALUES(?1,1,pcw_search_span()) ON CONFLICT(source) DO UPDATE SET revision=revision+1,span=excluded.span",[source])?;
    Ok(())
}
pub fn dirty(c: &Connection, limit: u16) -> Result<Vec<(String, i64)>> {
    dirty_for(c, None, limit)
}
pub fn dirty_for(c: &Connection, span: Option<&str>, limit: u16) -> Result<Vec<(String, i64)>> {
    let mut q=c.prepare("SELECT source,revision FROM organization_search_dirty WHERE(?1 IS NULL OR span=?1) ORDER BY source LIMIT ?2")?;
    Ok(
        q.query_map(params![span, i64::from(limit.min(BATCH))], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?,
    )
}

pub fn clear_dirty(c: &Connection, source: &str, revision: i64) -> Result<()> {
    transaction(c)?;
    if c.execute(
        "DELETE FROM organization_search_dirty WHERE source=?1 AND revision=?2",
        params![source, revision],
    )? != 1
    {
        return Err(bad());
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct State {
    pub generation: String,
    pub clock: i64,
    pub min_cut: i64,
    pub phase: i64,
    pub after: String,
    pub building: bool,
}
pub fn state(c: &Connection) -> Result<State> {
    let (generation, clock, min_cut, phase, after) = c.query_row(
        "SELECT generation,clock,min_cut,phase,after_key FROM organization_search_state WHERE id=1",
        [],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    )?;
    if clock < 0
        || min_cut < 0
        || min_cut > clock
        || !(0..=8).contains(&phase)
        || generation.len() != 32
    {
        return Err(bad());
    }
    let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM organization_search_dirty) OR EXISTS(SELECT 1 FROM organization_search_graph_jobs) OR EXISTS(SELECT 1 FROM organization_search_fanout) OR EXISTS(SELECT 1 FROM organization_search_gc)",[],|r|r.get(0))?;
    Ok(State {
        generation,
        clock,
        min_cut,
        phase,
        after,
        building: phase < 8 || pending,
    })
}
fn tick(c: &Connection) -> Result<i64> {
    transaction(c)?;
    let n = state(c)?.clock.checked_add(1).ok_or_else(bad)?;
    c.execute(
        "UPDATE organization_search_state SET clock=?1 WHERE id=1",
        [n],
    )?;
    Ok(n)
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub field: String,
    pub matched_on: MatchOn,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub revision: Option<String>,
    pub target: TargetRef,
    pub kind: Option<Kind>,
    pub owner_crew: Option<String>,
    pub table_id: Option<String>,
    pub channel_id: Option<String>,
    pub title: String,
    pub changed_at: Option<String>,
    pub status: Status,
    pub system_default: bool,
    pub slots: Vec<Slot>,
}
pub fn fingerprint(doc: &Document) -> Result<String> {
    Ok(crate::managed_links::hash(&serde_json::to_vec(doc)?))
}
pub fn rank(s: Status) -> i64 {
    match s {
        Status::Current => 0,
        Status::Historical => 1,
        Status::Archived => 2,
    }
}
#[derive(Default)]
pub struct Binding<'a> {
    pub object: Option<i64>,
    pub message: Option<&'a str>,
    pub artifact: Option<&'a str>,
    pub crew: Option<&'a str>,
    pub session: Option<&'a str>,
    pub channel: Option<&'a str>,
}
pub fn put(c: &Connection, source: &str, b: Binding<'_>, doc: &Document) -> Result<bool> {
    transaction(c)?;
    let target = key(&doc.target)?;
    let digest = fingerprint(doc)?;
    let old:Option<String>=c.query_row("SELECT fingerprint FROM organization_search_versions WHERE source=?1 AND valid_to IS NULL",[source],|r|r.get(0)).optional()?;
    if old.as_deref() == Some(&digest) {
        return Ok(false);
    }
    c.execute("INSERT INTO organization_search_sources VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(source) DO UPDATE SET target=excluded.target",params![source,target,b.object,b.message,b.artifact,b.crew,b.session,b.channel])?;
    let seq = tick(c)?;
    let stage:Option<i64>=c.query_row("SELECT v.id FROM organization_search_versions v JOIN organization_search_state st ON st.id=1 WHERE v.source=?1 AND v.valid_to IS NULL AND st.build_cut IS NOT NULL AND v.valid_from>st.build_cut",[source],|r|r.get(0)).optional()?;
    if let Some(version) = stage {
        c.execute(
            "DELETE FROM organization_search_slots WHERE version_id=?1",
            [version],
        )?;
        c.execute("UPDATE organization_search_versions SET valid_from=?2,kind=?3,owner_crew=?4,table_id=?5,channel_id=?6,changed_at=?7,status_rank=?8,system_default=?9,fingerprint=?10,document=?11 WHERE id=?1",params![version,seq,doc.kind.map(|k|serde_json::to_string(&k)).transpose()?,doc.owner_crew,doc.table_id,doc.channel_id,doc.changed_at,rank(doc.status),doc.system_default,digest,serde_json::to_string(doc)?])?;
        insert_slots(c, version, &doc.slots)?;
        return Ok(true);
    }
    c.execute(
        "UPDATE organization_search_versions SET valid_to=?2 WHERE source=?1 AND valid_to IS NULL",
        params![source, seq],
    )?;
    c.execute("INSERT INTO organization_search_versions(source,valid_from,kind,owner_crew,table_id,channel_id,changed_at,status_rank,system_default,fingerprint,document) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![source,seq,doc.kind.map(|k|serde_json::to_string(&k)).transpose()?,doc.owner_crew,doc.table_id,doc.channel_id,doc.changed_at,rank(doc.status),doc.system_default,digest,serde_json::to_string(doc)?])?;
    let id = c.last_insert_rowid();
    insert_slots(c, id, &doc.slots)?;
    schedule_gc(c, Some(source))?;
    Ok(true)
}
pub fn remove(c: &Connection, source: &str) -> Result<()> {
    transaction(c)?;
    c.execute(
        "DELETE FROM organization_search_sources WHERE source=?1",
        [source],
    )?;
    Ok(())
}
pub fn target(c: &Connection, source: &str) -> Result<Option<TargetRef>> {
    let v: Option<String> = c
        .query_row(
            "SELECT target FROM organization_search_sources WHERE source=?1",
            [source],
            |r| r.get(0),
        )
        .optional()?;
    v.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}

/// Bootstrap sources in actual indexed key order. New writes are independently dirty.
pub fn seed_step(c: &Connection, limit: u16) -> Result<usize> {
    transaction(c)?;
    let s = state(c)?;
    if s.phase == 8 {
        return Ok(0);
    }
    let limit = i64::from(limit.min(BATCH));
    if limit == 0 {
        return Ok(0);
    }
    let (sql, prefix) = match s.phase {
        0 => (
            "SELECT printf('%020d',number) FROM object_refs WHERE printf('%020d',number)>?1 ORDER BY number LIMIT ?2",
            "o:",
        ),
        1 => (
            "SELECT id FROM messages WHERE id>?1 ORDER BY id LIMIT ?2",
            "m:",
        ),
        2 => (
            "SELECT reference FROM managed_artifacts WHERE reference>?1 ORDER BY reference LIMIT ?2",
            "a:",
        ),
        3 => (
            "SELECT id FROM crews WHERE id>?1 ORDER BY id LIMIT ?2",
            "c:",
        ),
        4 => (
            "SELECT id FROM sessions WHERE id>?1 ORDER BY id LIMIT ?2",
            "s:",
        ),
        5 => (
            "SELECT id FROM channels WHERE id>?1 ORDER BY id LIMIT ?2",
            "h:",
        ),
        6 => {
            let mut q=c.prepare("SELECT event_id FROM managed_table_schema_events WHERE event_id>?1 ORDER BY event_id LIMIT ?2")?;
            let events = q
                .query_map(params![s.after, limit], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for event in &events {
                seed_alias(c, event)?;
            }
            if let Some(last) = events.last() {
                c.execute(
                    "UPDATE organization_search_state SET after_key=?1 WHERE id=1",
                    [last],
                )?;
            } else {
                c.execute(
                    "UPDATE organization_search_state SET phase=7,after_key='' WHERE id=1",
                    [],
                )?;
            }
            return Ok(events.len());
        }
        7 => {
            mark(c, "human")?;
            c.execute(
                "UPDATE organization_search_state SET phase=8,after_key='' WHERE id=1",
                [],
            )?;
            return Ok(1);
        }
        _ => return Err(bad()),
    };
    let mut q = c.prepare(sql)?;
    let keys: Vec<String> = q
        .query_map(params![s.after, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &keys {
        mark(c, &format!("{prefix}{id}"))?;
    }
    if let Some(last) = keys.last() {
        c.execute(
            "UPDATE organization_search_state SET after_key=?1 WHERE id=1",
            [last],
        )?;
    } else {
        c.execute(
            "UPDATE organization_search_state SET phase=phase+1,after_key='' WHERE id=1",
            [],
        )?;
    }
    Ok(keys.len())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub rank: i64,
    pub at: String,
    pub seq: i64,
    pub source: String,
}
#[derive(Clone, Debug)]
pub struct Candidate {
    pub source: String,
    pub document: Document,
    pub position: Position,
}
pub fn candidates(
    c: &Connection,
    cut: i64,
    after: Option<&Position>,
    limit: u16,
    filter: &str,
    values: Vec<rusqlite::types::Value>,
) -> Result<Vec<Candidate>> {
    let (r, t, n, k) = after
        .map(|p| (p.rank, p.at.clone(), p.seq, p.source.clone()))
        .unwrap_or((-1, String::new(), i64::MAX, String::new()));
    let sql = format!(
        "SELECT v.source,v.document,v.status_rank,COALESCE(v.changed_at,''),v.valid_from FROM organization_search_versions v JOIN organization_search_sources src ON src.source=v.source WHERE v.valid_from<=?1 AND(v.valid_to IS NULL OR v.valid_to>?1) AND(v.status_rank>?2 OR(v.status_rank=?2 AND(COALESCE(v.changed_at,'')<?3 OR(COALESCE(v.changed_at,'')=?3 AND(v.valid_from<?4 OR(v.valid_from=?4 AND v.source>?5)))))) AND ({filter}) ORDER BY v.status_rank,COALESCE(v.changed_at,'') DESC,v.valid_from DESC,v.source LIMIT ?6"
    );
    let mut q = c.prepare(&sql)?;
    let mut args: Vec<rusqlite::types::Value> = vec![
        cut.into(),
        r.into(),
        t.into(),
        n.into(),
        k.into(),
        i64::from(limit).into(),
    ];
    args.extend(values);
    let raw = q
        .query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(source, body, rank, at, seq)| {
            Ok(Candidate {
                position: Position {
                    rank,
                    at,
                    seq,
                    source: source.clone(),
                },
                source,
                document: serde_json::from_str(&body)?,
            })
        })
        .collect()
}
pub fn current(c: &Connection, source: &str) -> Result<Option<Document>> {
    let body:Option<String>=c.query_row("SELECT document FROM organization_search_versions WHERE source=?1 AND valid_to IS NULL",[source],|r|r.get(0)).optional()?;
    body.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}

/// Every discarded closed interval advances a private safety floor, never silent holes.
fn insert_slots(c: &Connection, id: i64, slots: &[Slot]) -> Result<()> {
    for slot in slots {
        c.execute("INSERT INTO organization_search_slots(version_id,field_id,slot_kind,body) VALUES(?1,?2,?3,?4)",params![id,slot.field,serde_json::to_string(&slot.matched_on)?,slot.text])?;
    }
    Ok(())
}
fn schedule_gc(c: &Connection, source: Option<&str>) -> Result<()> {
    schedule_gc_with(c, source, HISTORY_BYTES)
}
fn schedule_gc_with(c: &Connection, source: Option<&str>, byte_limit: i64) -> Result<()> {
    let mut candidates = Vec::new();
    if let Some(source) = source {
        let row:Option<(i64,i64)>=c.query_row("SELECT id,valid_to FROM organization_search_versions WHERE source=?1 AND valid_to IS NOT NULL ORDER BY valid_to DESC,id DESC LIMIT 1 OFFSET ?2",params![source,CLOSED_PER_SOURCE],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        candidates.extend(row);
    }
    let (closed, bytes): (i64, i64) = c.query_row(
        "SELECT closed_count,history_bytes FROM organization_search_state WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if closed > CLOSED_TOTAL || bytes > byte_limit {
        let row:Option<(i64,i64)>=c.query_row("SELECT id,valid_to FROM organization_search_versions WHERE valid_to IS NOT NULL ORDER BY valid_to,id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        candidates.extend(row);
    }
    for (id, to) in candidates {
        c.execute("UPDATE organization_search_state SET min_cut=max(min_cut,?1),build_cut=COALESCE(build_cut,clock) WHERE id=1",[to])?;
        c.execute(
            "INSERT INTO organization_search_gc VALUES(?1,?2,pcw_search_span()) ON CONFLICT DO NOTHING",
            params![id, to],
        )?;
    }
    Ok(())
}
/// At most budget actual child/header facts. Cut retirement happens before deletion.
pub fn gc_step(c: &Connection, budget: u16) -> Result<usize> {
    gc_step_for(c, None, budget)
}
pub fn gc_step_for(c: &Connection, span: Option<&str>, budget: u16) -> Result<usize> {
    transaction(c)?;
    let mut remaining = usize::from(budget.min(BATCH));
    let initial = remaining;
    while remaining > 0 {
        let id:Option<i64>=c.query_row("SELECT version_id FROM organization_search_gc WHERE(?1 IS NULL OR span=?1) ORDER BY valid_to,version_id LIMIT 1",[span],|r|r.get(0)).optional()?;
        let Some(id) = id else { break };
        let mut q = c.prepare(
            "SELECT id FROM organization_search_slots WHERE version_id=?1 ORDER BY id LIMIT ?2",
        )?;
        let slots = q
            .query_map(params![id, remaining as i64], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for slot in &slots {
            c.execute("DELETE FROM organization_search_slots WHERE id=?1", [slot])?;
        }
        remaining -= slots.len();
        if slots.is_empty() {
            c.execute("DELETE FROM organization_search_versions WHERE id=?1", [id])?;
            remaining -= 1;
            schedule_gc(c, None)?;
        }
    }
    Ok(initial - remaining)
}
pub fn prune(c: &Connection) -> Result<usize> {
    schedule_gc(c, None)?;
    Ok(0)
}

pub fn restore(c: &Connection) -> Result<()> {
    if enabled(c)? {
        transaction(c)?;
        c.execute(
            "UPDATE organization_search_state SET generation=lower(hex(randomblob(16))) WHERE id=1",
            [],
        )?;
    }
    Ok(())
}

pub fn graph_job(c: &Connection, source: &str, fingerprint: &str) -> Result<()> {
    transaction(c)?;
    c.execute("INSERT INTO organization_search_graph_jobs VALUES(?1,?2,NULL,?3,pcw_search_span()) ON CONFLICT(source) DO UPDATE SET fingerprint=excluded.fingerprint,after_key=NULL,seen_generation=excluded.seen_generation,span=excluded.span",params![source,fingerprint,tick(c)?])?;
    Ok(())
}
/// Source, fingerprint, continuation key and the current retained generation.
pub type GraphJob = (String, String, Option<String>, i64);
pub fn graph_jobs(c: &Connection, limit: u16) -> Result<Vec<GraphJob>> {
    graph_jobs_for(c, None, limit)
}
pub fn graph_jobs_for(c: &Connection, span: Option<&str>, limit: u16) -> Result<Vec<GraphJob>> {
    let mut q=c.prepare("SELECT j.source,j.fingerprint,j.after_key,j.seen_generation FROM organization_search_graph_jobs j WHERE(?1 IS NULL OR j.span=?1) AND NOT EXISTS(SELECT 1 FROM organization_search_dirty d WHERE d.source=j.source) ORDER BY j.source LIMIT ?2")?;
    Ok(
        q.query_map(params![span, i64::from(limit.min(BATCH))], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?,
    )
}

pub fn graph_page(
    c: &Connection,
    source: &str,
    stamp: i64,
    edges: &[EdgeView],
    next: Option<&str>,
) -> Result<()> {
    transaction(c)?;
    for edge in edges {
        let edge_key = format!("{}:{}", source, edge.id);
        let target = key(&edge.target)?;
        let mut fact = edge.clone();
        fact.summary.clear();
        fact.editable = false;
        let body = serde_json::to_string(&fact)?;
        let target_source: String = c.query_row(
            "SELECT source FROM organization_search_sources WHERE target=?1",
            [&target],
            |r| r.get(0),
        )?;
        let old: Option<String> = c
            .query_row(
                "SELECT body FROM organization_search_edges WHERE edge_key=?1 AND valid_to IS NULL",
                [&edge_key],
                |r| r.get(0),
            )
            .optional()?;
        if old.as_deref() == Some(&body) {
            c.execute("UPDATE organization_search_edges SET seen_stamp=?2 WHERE edge_key=?1 AND valid_to IS NULL",params![edge_key,stamp])?;
            continue;
        }
        let seq = tick(c)?;
        c.execute(
            "DELETE FROM organization_search_edges WHERE edge_key=?1",
            [&edge_key],
        )?;
        c.execute("INSERT INTO organization_search_edges(edge_key,source,target,valid_from,body,seen_stamp,owner_crew,target_source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![edge_key,source,target,seq,body,stamp,edge.owner_crew_id,target_source])?;
    }
    // A separate, credit-limited cleanup stage retires unseen facts.
    if let Some(next) = next
        && c.execute("UPDATE organization_search_graph_jobs SET after_key=?2 WHERE source=?1 AND seen_generation=?3",params![source,next,stamp])? != 1
    {
        return Err(bad());
    }
    Ok(())
}
pub fn graph_cleanup(
    c: &Connection,
    source: &str,
    stamp: i64,
    budget: u16,
) -> Result<(usize, bool)> {
    transaction(c)?;
    let budget = budget.min(BATCH);
    if budget == 0 {
        return Ok((0, false));
    }
    let current: Option<i64> = c
        .query_row(
            "SELECT seen_generation FROM organization_search_graph_jobs WHERE source=?1",
            [source],
            |r| r.get(0),
        )
        .optional()?;
    if current != Some(stamp) {
        return Err(bad());
    }
    let mut q=c.prepare("SELECT id FROM organization_search_edges WHERE source=?1 AND seen_stamp!=?2 ORDER BY id LIMIT ?3")?;
    let ids = q
        .query_map(params![source, stamp, i64::from(budget)], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in &ids {
        c.execute("DELETE FROM organization_search_edges WHERE id=?1", [id])?;
    }
    let complete = ids.len() < usize::from(budget);
    if complete {
        c.execute(
            "DELETE FROM organization_search_graph_jobs WHERE source=?1 AND seen_generation=?2",
            params![source, stamp],
        )?;
    }
    Ok((ids.len(), complete))
}

pub fn graph_candidates(
    c: &Connection,
    target: &TargetRef,
    cut: i64,
    after: i64,
    limit: u16,
) -> Result<Vec<(i64, String, EdgeView)>> {
    let target = key(target)?;
    let mut q=c.prepare("SELECT e.id,e.source,e.body FROM organization_search_edges e JOIN organization_search_sources s ON s.source=e.source WHERE(e.target=?1 OR s.target=?1) AND e.id>?2 AND e.valid_from<=?3 AND(e.valid_to IS NULL OR e.valid_to>?3) ORDER BY e.id LIMIT ?4")?;
    q.query_map(params![target, after, cut, i64::from(limit)], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?
    .into_iter()
    .map(|(id, source, body)| Ok((id, source, serde_json::from_str(&body)?)))
    .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphPosition {
    pub position: Position,
    pub id: i64,
}
pub struct GraphCandidate {
    pub position: GraphPosition,
    pub edge: EdgeView,
    pub neighbor: TargetRef,
}
pub fn ordered_graph(
    c: &Connection,
    target: &TargetRef,
    cut: i64,
    after: Option<&GraphPosition>,
    limit: u16,
    filter: &str,
    values: Vec<rusqlite::types::Value>,
) -> Result<Vec<GraphCandidate>> {
    let (r, t, n, k, id) = after
        .map(|p| {
            (
                p.position.rank,
                p.position.at.clone(),
                p.position.seq,
                p.position.source.clone(),
                p.id,
            )
        })
        .unwrap_or((-1, String::new(), i64::MAX, String::new(), 0));
    let sql = format!(
        "WITH related AS(SELECT e.id,e.source,e.body,e.edge_key,e.valid_from,s.target AS from_target,e.target AS to_target,CASE WHEN s.target=?1 THEN e.target ELSE s.target END AS neighbor FROM organization_search_edges e JOIN organization_search_sources s ON s.source=e.source WHERE(s.target=?1 OR e.target=?1) AND e.valid_from<=?2 AND(e.valid_to IS NULL OR e.valid_to>?2) AND EXISTS(SELECT 1 FROM organization_search_edges live WHERE live.edge_key=e.edge_key AND live.valid_from=e.valid_from AND live.valid_to IS NULL)) SELECT e.id,e.body,n.source,v.status_rank,COALESCE(v.changed_at,''),v.valid_from,e.neighbor FROM related e JOIN organization_search_sources src ON src.source=e.source JOIN organization_search_sources n ON n.target=e.neighbor JOIN organization_search_versions v ON v.source=n.source AND v.valid_from<=?2 AND(v.valid_to IS NULL OR v.valid_to>?2) WHERE(v.status_rank>?3 OR(v.status_rank=?3 AND(COALESCE(v.changed_at,'')<?4 OR(COALESCE(v.changed_at,'')=?4 AND(v.valid_from<?5 OR(v.valid_from=?5 AND(n.source>?6 OR(n.source=?6 AND e.id>?7)))))))) AND ({filter}) ORDER BY v.status_rank,COALESCE(v.changed_at,'') DESC,v.valid_from DESC,n.source,e.id LIMIT ?8"
    );
    let mut q = c.prepare(&sql)?;
    let mut args: Vec<rusqlite::types::Value> = vec![
        key(target)?.into(),
        cut.into(),
        r.into(),
        t.into(),
        n.into(),
        k.into(),
        id.into(),
        i64::from(limit).into(),
    ];
    args.extend(values);
    let raw = q
        .query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(id, body, source, rank, at, seq, neighbor)| {
            Ok(GraphCandidate {
                position: GraphPosition {
                    position: Position {
                        rank,
                        at,
                        seq,
                        source,
                    },
                    id,
                },
                edge: serde_json::from_str(&body)?,
                neighbor: serde_json::from_str(&neighbor)?,
            })
        })
        .collect()
}

/// Called by the actual organization save, after a real contiguous event.
pub fn mark_work_event(
    c: &Connection,
    state: &crewd_work::BookState,
    event: &crewd_work::Event,
) -> Result<()> {
    if !enabled(c)? {
        return Ok(());
    }
    use crewd_work::EventKind::*;
    let (kind, id, work) = match &event.kind {
        EntryResolution { entry, .. }
        | EntryRegistered { entry }
        | EntryClaimed { entry, .. }
        | EntryClosed { entry, .. } => ("requirement", entry.to_string(), None),
        AskOpened { ask, .. }
        | AskAnswered { ask, .. }
        | AskDefaulted { ask, .. }
        | AskWithdrawn { ask, .. }
        | EscalationNoted { ask, .. } => ("human_task", ask.to_string(), None),
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
        | CheckpointWake { id, .. } => ("plan", id.to_string(), Some(*id)),
    };
    let n: Option<i64> = c
        .query_row(
            "SELECT number FROM object_refs WHERE kind=?1 AND core_id=?2",
            params![kind, id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(n) = n {
        mark_at(c, &source_object(n), &crate::ts_from_ms(event.at.0))?;
    }
    if let Some(w) = work.and_then(|id| state.items.get(&id)) {
        for entry in &w.sources {
            let n: Option<i64> = c
                .query_row(
                    "SELECT number FROM object_refs WHERE kind='requirement' AND core_id=?1",
                    [entry.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(n) = n {
                mark_at(c, &source_object(n), &crate::ts_from_ms(event.at.0))?;
            }
        }
    }
    Ok(())
}

pub fn seed_alias(c: &Connection, event: &str) -> Result<()> {
    transaction(c)?;
    c.execute("INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT table_number,event_id,'',json_extract(before_json,'$.title'),0 FROM managed_table_schema_events WHERE event_id=?1 AND json_extract(before_json,'$.title')!=json_extract(after_json,'$.title') ON CONFLICT DO NOTHING",[event])?;
    c.execute("INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT table_number,event_id,'@purpose',json_extract(before_json,'$.purpose'),0 FROM managed_table_schema_events WHERE event_id=?1 AND json_extract(before_json,'$.purpose') IS NOT NULL AND json_extract(before_json,'$.purpose') IS NOT json_extract(after_json,'$.purpose') ON CONFLICT DO NOTHING",[event])?;
    c.execute("INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT e.table_number,e.event_id,json_extract(b.value,'$.id'),json_extract(b.value,'$.name'),0 FROM managed_table_schema_events e,json_each(e.before_json,'$.fields') b JOIN json_each(e.after_json,'$.fields') a ON json_extract(a.value,'$.id')=json_extract(b.value,'$.id') WHERE e.event_id=?1 AND json_extract(a.value,'$.name')!=json_extract(b.value,'$.name') ON CONFLICT DO NOTHING",[event])?;
    Ok(())
}
pub fn alias_match(c: &Connection, n: i64, query: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT label FROM organization_search_aliases WHERE table_number=?1 AND instr(lower(label),lower(?2))>0 ORDER BY id LIMIT 1",params![n,query],|r|r.get(0)).optional()?)
}

pub fn fanout_step(c: &Connection, limit: u16) -> Result<usize> {
    transaction(c)?;
    let job:Option<(String,String,i64)>=c.query_row("SELECT crew_id,after_key,revision FROM organization_search_fanout ORDER BY crew_id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((crew, after, revision)) = job else {
        return Ok(0);
    };
    let mut q=c.prepare("SELECT source FROM organization_search_versions WHERE owner_crew=?1 AND valid_to IS NULL AND source>?2 ORDER BY source LIMIT ?3")?;
    let keys = q
        .query_map(params![crew, after, i64::from(limit.min(BATCH))], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for key in &keys {
        mark(c, key)?;
    }
    if let Some(last) = keys.last() {
        c.execute(
            "UPDATE organization_search_fanout SET after_key=?2 WHERE crew_id=?1 AND revision=?3",
            params![crew, last, revision],
        )?;
    } else {
        c.execute(
            "DELETE FROM organization_search_fanout WHERE crew_id=?1 AND revision=?2",
            params![crew, revision],
        )?;
    }
    Ok(keys.len())
}

fn queue_sql(key: &str, unknown_on_raw: bool) -> String {
    let unknown = if unknown_on_raw {
        format!(
            "INSERT INTO organization_search_times SELECT {key},NULL,1 FROM organization_search_state WHERE id=1 AND pcw_search_wall() IS NULL ON CONFLICT(source) DO UPDATE SET changed_at=NULL,revision=revision+1;"
        )
    } else {
        String::new()
    };
    format!(
        "UPDATE organization_search_state SET build_cut=COALESCE(build_cut,clock) WHERE id=1; INSERT INTO organization_search_dirty VALUES({key},1,pcw_search_span()) ON CONFLICT(source) DO UPDATE SET revision=revision+1,span=excluded.span; {unknown} INSERT INTO organization_search_times SELECT {key},pcw_search_wall(),1 FROM organization_search_state WHERE id=1 AND pcw_search_wall() IS NOT NULL ON CONFLICT(source) DO UPDATE SET changed_at=excluded.changed_at,revision=revision+1;"
    )
}

fn changed_columns(c: &Connection, table: &str) -> Result<String> {
    let mut q = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols = q
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let cols: Vec<String> = if table == "messages" {
        cols.into_iter()
            .filter(|n| n == "text" || n == "channel_id")
            .collect()
    } else if table == "managed_table_attributes" {
        // Restore carries an issued-ID floor, not a business schema mutation.
        cols.into_iter()
            .filter(|n| n != "next_field_number")
            .collect()
    } else {
        cols
    };
    if cols.is_empty() {
        return Err(bad());
    }
    Ok(cols
        .into_iter()
        .map(|s| {
            format!(
                "OLD.\"{}\" IS NOT NEW.\"{}\"",
                s.replace('"', "\"\""),
                s.replace('"', "\"\"")
            )
        })
        .collect::<Vec<_>>()
        .join(" OR "))
}
/// Close a publication barrier only after an explicit indexing step did work.
/// Do not run this for a no-op business transaction: even an equal-value UPDATE
/// changes SQLite's physical total_changes counter.
pub fn finish_publication(c: &Connection) -> Result<()> {
    if !state(c)?.building {
        c.execute("UPDATE organization_search_state SET build_cut=NULL WHERE id=1 AND build_cut IS NOT NULL",[])?;
    }
    Ok(())
}
pub fn mark_at(c: &Connection, source: &str, at: &str) -> Result<()> {
    if !enabled(c)? {
        return Ok(());
    }
    mark(c, source)?;
    c.execute("INSERT INTO organization_search_times VALUES(?1,?2,1) ON CONFLICT(source) DO UPDATE SET changed_at=excluded.changed_at,revision=revision+1",params![source,at])?;
    Ok(())
}
pub fn mutation_time(c: &Connection, source: &str) -> Result<Option<(Option<String>, i64)>> {
    Ok(c.query_row(
        "SELECT changed_at,revision FROM organization_search_times WHERE source=?1",
        [source],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

pub fn ensure_source(
    c: &Connection,
    source: &str,
    b: Binding<'_>,
    target: &TargetRef,
) -> Result<bool> {
    transaction(c)?;
    let n=c.execute("INSERT INTO organization_search_sources VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(source) DO NOTHING",params![source,key(target)?,b.object,b.message,b.artifact,b.crew,b.session,b.channel])?;
    let exact: Option<String> = c
        .query_row(
            "SELECT target FROM organization_search_sources WHERE source=?1",
            [source],
            |r| r.get(0),
        )
        .optional()?;
    if exact.as_deref() != Some(&key(target)?) {
        return Err(bad());
    }
    if n != 0 {
        mark(c, source)?;
    }
    Ok(n != 0)
}

#[derive(Debug)]
pub struct PayloadUsage {
    pub current_bytes: i64,
    pub stage_bytes: i64,
    pub history_bytes: i64,
}
pub fn payload_usage(c: &Connection) -> Result<PayloadUsage> {
    let history_bytes: i64 = c.query_row(
        "SELECT history_bytes FROM organization_search_state WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let document_current:i64=c.query_row("SELECT COALESCE(sum(length(CAST(v.document AS BLOB))+COALESCE((SELECT sum(length(CAST(s.body AS BLOB))) FROM organization_search_slots s WHERE s.version_id=v.id),0)),0) FROM organization_search_versions v JOIN organization_search_state st ON st.id=1 WHERE v.valid_to IS NULL AND(st.build_cut IS NULL OR v.valid_from<=st.build_cut)",[],|r|r.get(0))?;
    let document_stage:i64=c.query_row("SELECT COALESCE(sum(length(CAST(v.document AS BLOB))+COALESCE((SELECT sum(length(CAST(s.body AS BLOB))) FROM organization_search_slots s WHERE s.version_id=v.id),0)),0) FROM organization_search_versions v JOIN organization_search_state st ON st.id=1 WHERE v.valid_to IS NULL AND st.build_cut IS NOT NULL AND v.valid_from>st.build_cut",[],|r|r.get(0))?;
    let edges_current:i64=c.query_row("SELECT COALESCE(sum(length(CAST(e.body AS BLOB))),0) FROM organization_search_edges e WHERE NOT EXISTS(SELECT 1 FROM organization_search_graph_jobs j WHERE j.source=e.source)",[],|r|r.get(0))?;
    let edges_stage:i64=c.query_row("SELECT COALESCE(sum(length(CAST(e.body AS BLOB))),0) FROM organization_search_edges e WHERE EXISTS(SELECT 1 FROM organization_search_graph_jobs j WHERE j.source=e.source)",[],|r|r.get(0))?;
    Ok(PayloadUsage {
        current_bytes: document_current
            .checked_add(edges_current)
            .ok_or_else(bad)?,
        stage_bytes: document_stage.checked_add(edges_stage).ok_or_else(bad)?,
        history_bytes,
    })
}
pub fn gc_pending(c: &Connection) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM organization_search_gc)",
        [],
        |r| r.get(0),
    )?)
}

pub fn span(c: &Connection) -> Result<Option<String>> {
    Ok(c.query_row("SELECT pcw_search_span()", [], |r| r.get(0))?)
}

#[cfg(test)]
#[path = "organization_search_tests.rs"]
mod tests;
