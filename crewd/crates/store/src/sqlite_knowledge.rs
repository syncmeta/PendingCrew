//! Per-crew SQLite knowledge. Core ACL and work facts never enter this database.
//! Object mutations and immutable original-request receipts commit together.
use crate::{Result, StoreError};
use crewd_proto::team_data::*;
use rusqlite::{
    Connection, OptionalExtension,
    hooks::{AuthAction, AuthContext, Authorization},
    params,
    types::{Value as SqlValue, ValueRef},
};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn invalid(s: impl Into<String>) -> StoreError {
    StoreError::Invalid(s.into())
}
pub fn digest(v: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(v)?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub fn namespace(db: &str, crew: &str) -> String {
    digest(&(db, crew)).expect("string pair serialization")
}
#[derive(Clone, Debug)]
pub enum Allocation {
    Object {
        alias: String,
        kind: String,
    },
    Placement {
        alias: String,
        parent: PlacementParent,
    },
}
#[derive(Clone, Debug)]
pub enum PlacementParent {
    Created(String),
    Existing(DataObjectVersion),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capture {
    pub anchor: String,
    pub namespace: String,
    pub kind: String,
    pub id: String,
    pub commit: String,
    pub revision: String,
    pub live: bool,
}
#[derive(Clone, Debug)]
pub enum Mutation {
    PutObject {
        object: DataObject,
        expected_revision: Option<String>,
        captures: Vec<Capture>,
    },
    Sql {
        sql: String,
        params: Vec<DataCell>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stored {
    pub object: DataObject,
    pub captures: Vec<Capture>,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    principal: String,
    digest: String,
    origin: DataContext,
    result: DataChangeResult,
}
#[derive(Serialize, Deserialize)]
struct PreviewRecord {
    principal: String,
    preview: DataPreview,
}
pub struct Knowledge {
    pub conn: Connection,
    pub before_commit: Option<Box<dyn Fn() -> Result<()> + Send>>,
    path: PathBuf,
    base: DataContext,
}
const DDL: &str = "
CREATE TABLE IF NOT EXISTS _pc_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS _pc_objects(kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,archived INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id));
CREATE TABLE IF NOT EXISTS _pc_revisions(kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id,revision));
CREATE TABLE IF NOT EXISTS _pc_receipts(id TEXT PRIMARY KEY,body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS _pc_previews(id TEXT PRIMARY KEY,body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS _pc_snapshots(id TEXT PRIMARY KEY,body TEXT NOT NULL);
";
pub fn private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || std::fs::canonicalize(path)? != path
    {
        return Err(invalid("private knowledge directory required"));
    }
    Ok(())
}
fn private_file(path: &Path) -> Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
    {
        return Err(invalid("private non-aliased knowledge file required"));
    }
    Ok(())
}
pub fn object_fields(o: &DataObject) -> (&str, &str, &str, bool) {
    match o {
        DataObject::Block {
            id,
            revision,
            archived,
            ..
        } => ("block", id, revision, *archived),
        DataObject::View {
            id,
            revision,
            archived,
            ..
        } => ("view", id, revision, *archived),
        DataObject::Relation {
            id,
            revision,
            archived,
            ..
        } => ("relation", id, revision, *archived),
    }
}
impl Knowledge {
    pub fn open(path: &Path, base: DataContext, create: bool) -> Result<Self> {
        let new_file = !path.exists();
        if new_file {
            if !create {
                return Err(invalid("knowledge workspace is uninitialized"));
            }
            private_dir(
                path.parent()
                    .ok_or_else(|| invalid("knowledge parent missing"))?,
            )?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
        }
        private_file(path)?;
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_millis(250))?;
        if !new_file {
            let app: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
            let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            if app != 0x50434b31 || version != 1 {
                return Err(invalid(
                    "unknown knowledge SQLite format; original file retained",
                ));
            }
        }
        conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        if new_file {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(DDL)?;
            tx.execute_batch("PRAGMA application_id=1346587441; PRAGMA user_version=1;")?;
            tx.execute(
                "INSERT INTO _pc_meta VALUES('context',?1)",
                [serde_json::to_string(&base)?],
            )?;
            tx.commit()?;
        }
        conn.set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
            2 * 1024 * 1024,
        )?;
        conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, 65536)?;
        conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_COLUMN, 128)?;
        conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 256)?;
        conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_EXPR_DEPTH, 100)?;
        conn.execute_batch("PRAGMA max_page_count=65536;")?;
        let this = Self {
            conn,
            before_commit: None,
            path: path.into(),
            base,
        };
        let current = this.context()?;
        if current.db_id != this.base.db_id
            || current.authority_id != this.base.authority_id
            || current.crew_id != this.base.crew_id
            || current.namespace != this.base.namespace
        {
            return Err(invalid("knowledge namespace identity differs"));
        }
        Ok(this)
    }
    pub fn context(&self) -> Result<DataContext> {
        let raw: String =
            self.conn
                .query_row("SELECT value FROM _pc_meta WHERE key='context'", [], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&raw)?)
    }
    pub fn check(&self, c: &DataContext) -> Result<()> {
        if self.context()? != *c {
            return Err(invalid("knowledge generation or restore context changed"));
        }
        Ok(())
    }
    pub fn get(&self, r: &DataRef, revision: Option<&str>) -> Result<Stored> {
        if r.namespace != self.base.namespace {
            return Err(invalid("cross-crew reference unavailable"));
        }
        let raw: Option<String> = if let Some(revision) = revision {
            let rev = revision
                .parse::<i64>()
                .map_err(|_| invalid("invalid revision"))?;
            self.conn
                .query_row(
                    "SELECT body FROM _pc_revisions WHERE kind=?1 AND id=?2 AND revision=?3",
                    params![r.kind, r.id, rev],
                    |row| row.get(0),
                )
                .optional()?
        } else {
            self.conn
                .query_row(
                    "SELECT body FROM _pc_objects WHERE kind=?1 AND id=?2",
                    params![r.kind, r.id],
                    |row| row.get(0),
                )
                .optional()?
        };
        Ok(serde_json::from_str(
            &raw.ok_or_else(|| invalid("knowledge object unavailable"))?,
        )?)
    }
    pub fn list(
        &self,
        kind: &str,
        after: Option<&str>,
        limit: u32,
        deleted: bool,
    ) -> Result<(Vec<DataObject>, Option<String>)> {
        bound(limit, 100)?;
        let mut q=self.conn.prepare("SELECT id,body FROM _pc_objects WHERE kind=?1 AND (?2 IS NULL OR id>?2) AND (?3 OR archived=0) ORDER BY id LIMIT ?4")?;
        let rows = q
            .query_map(params![kind, after, deleted, limit + 1], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next = if rows.len() > limit as usize {
            rows.get(limit as usize - 1).map(|r| r.0.clone())
        } else {
            None
        };
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(|(_, raw)| serde_json::from_str::<Stored>(&raw).map(|s| s.object))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok((items, next))
    }
    pub fn history(
        &self,
        r: &DataRef,
        after: Option<&str>,
        limit: u32,
    ) -> Result<(Vec<DataObject>, Option<String>)> {
        bound(limit, 100)?;
        if r.namespace != self.base.namespace {
            return Err(invalid("reference unavailable"));
        }
        let after = after
            .map(|s| {
                s.parse::<i64>()
                    .map_err(|_| invalid("invalid history cursor"))
            })
            .transpose()?
            .unwrap_or(0);
        let mut q=self.conn.prepare("SELECT revision,body FROM _pc_revisions WHERE kind=?1 AND id=?2 AND revision>?3 ORDER BY revision LIMIT ?4")?;
        let rows = q
            .query_map(params![r.kind, r.id, after, limit + 1], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next = if rows.len() > limit as usize {
            Some(rows[limit as usize - 1].0.to_string())
        } else {
            None
        };
        Ok((
            rows.into_iter()
                .take(limit as usize)
                .map(|(_, s)| serde_json::from_str::<Stored>(&s).map(|s| s.object))
                .collect::<std::result::Result<Vec<_>, _>>()?,
            next,
        ))
    }
    /// A Human can inspect an original request under current ACL; an Agent can only
    /// inspect its own. Origin equality stays mandatory for both identities.
    pub fn lookup(
        &self,
        id: &str,
        origin: &DataContext,
        principal: &str,
        human: bool,
    ) -> Result<DataRequestLookup> {
        valid_uuid(id)?;
        let raw: Option<String> = self
            .conn
            .query_row("SELECT body FROM _pc_receipts WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(raw) = raw else {
            return Ok(DataRequestLookup::NotFound);
        };
        let receipt: Receipt = serde_json::from_str(&raw)?;
        if receipt.origin != *origin || (!human && receipt.principal != principal) {
            return Err(invalid(
                "original request owner or immutable context differs",
            ));
        }
        let current = self.context()?;
        Ok(if current.restore_epoch != origin.restore_epoch {
            DataRequestLookup::Historical(receipt.result)
        } else {
            DataRequestLookup::Known(receipt.result)
        })
    }
    pub fn allocate(&self, allocations: &[Allocation]) -> Result<BTreeMap<String, DataCreated>> {
        let mut ids = BTreeMap::new();
        let mut names = BTreeSet::new();
        for a in allocations {
            let name = match a {
                Allocation::Object { alias, .. } | Allocation::Placement { alias, .. } => alias,
            };
            if name.is_empty() || name.len() > 64 || !names.insert(name) {
                return Err(invalid("duplicate or invalid alias"));
            }
        }
        for a in allocations {
            if let Allocation::Object { alias, kind } = a {
                ids.insert(
                    alias.clone(),
                    DataCreated::Object {
                        object: DataObjectVersion {
                            reference: DataRef {
                                namespace: self.base.namespace.clone(),
                                kind: kind.clone(),
                                id: uuid::Uuid::new_v4().to_string(),
                            },
                            revision: "1".into(),
                        },
                    },
                );
            }
        }
        for a in allocations {
            if let Allocation::Placement { alias, parent } = a {
                let parent = match parent {
                    PlacementParent::Existing(v) => v.clone(),
                    PlacementParent::Created(alias) => match ids.get(alias) {
                        Some(DataCreated::Object { object }) => object.clone(),
                        _ => return Err(invalid("placement parent unavailable")),
                    },
                };
                ids.insert(
                    alias.clone(),
                    DataCreated::Placement {
                        id: uuid::Uuid::new_v4().to_string(),
                        parent_view: parent,
                    },
                );
            }
        }
        Ok(ids)
    }
    fn existing(
        &self,
        id: &str,
        origin: &DataContext,
        principal: &str,
        payload: &str,
    ) -> Result<Option<DataChangeResult>> {
        valid_uuid(id)?;
        let raw: Option<String> = self
            .conn
            .query_row("SELECT body FROM _pc_receipts WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(raw) = raw else { return Ok(None) };
        let receipt: Receipt = serde_json::from_str(&raw)?;
        if receipt.origin != *origin || receipt.principal != principal || receipt.digest != payload
        {
            return Err(invalid(
                "request immutable principal, context or payload conflict",
            ));
        }
        let mut result = receipt.result;
        if self.context()?.restore_epoch != origin.restore_epoch {
            result.phase = DataPhase::Historical;
        }
        Ok(Some(result))
    }
    pub fn retry(
        &self,
        id: &str,
        origin: &DataContext,
        principal: &str,
        payload: &str,
    ) -> Result<Option<DataChangeResult>> {
        self.existing(id, origin, principal, payload)
    }
    pub fn apply(
        &mut self,
        id: &str,
        origin: &DataContext,
        principal: &str,
        payload: &str,
        created: BTreeMap<String, DataCreated>,
        mutations: Vec<Mutation>,
        confirmed: bool,
        snapshot: Option<String>,
    ) -> Result<DataChangeResult> {
        if let Some(result) = self.existing(id, origin, principal, payload)? {
            return Ok(result);
        }
        self.check(origin)?;
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        for mutation in mutations {
            if Instant::now() >= deadline {
                return Err(invalid("knowledge batch deadline exceeded"));
            }
            match mutation {
                Mutation::PutObject {
                    object,
                    expected_revision,
                    captures,
                } => {
                    let (kind, key, revision, archived) = object_fields(&object);
                    valid_uuid(key)?;
                    let current:Option<String>=tx.query_row("SELECT CAST(revision AS TEXT) FROM _pc_objects WHERE kind=?1 AND id=?2",params![kind,key],|r|r.get(0)).optional()?;
                    if current != expected_revision {
                        return Err(invalid("object revision conflict"));
                    }
                    let rev = revision
                        .parse::<i64>()
                        .map_err(|_| invalid("invalid object revision"))?;
                    if rev
                        != current
                            .as_deref()
                            .unwrap_or("0")
                            .parse::<i64>()
                            .map_err(|_| invalid("invalid stored revision"))?
                            .checked_add(1)
                            .ok_or_else(|| invalid("revision exhausted"))?
                    {
                        return Err(invalid("revision must advance linearly"));
                    }
                    let reference = DataRef {
                        namespace: origin.namespace.clone(),
                        kind: kind.into(),
                        id: key.into(),
                    };
                    let raw = serde_json::to_string(&Stored {
                        object: object.clone(),
                        captures,
                    })?;
                    if raw.len() > 1024 * 1024 {
                        return Err(invalid("object byte bound exceeded"));
                    }
                    tx.execute(
                        "INSERT INTO _pc_revisions VALUES(?1,?2,?3,?4)",
                        params![kind, key, rev, raw],
                    )?;
                    tx.execute("INSERT INTO _pc_objects VALUES(?1,?2,?3,?4,?5) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,archived=excluded.archived,body=excluded.body",params![kind,key,rev,archived,raw])?;
                    changed.push(DataObjectVersion {
                        reference,
                        revision: revision.into(),
                    });
                }
                Mutation::Sql { sql, params } => {
                    run_sql(&tx, &DataSql { sql, params }, false, confirmed, 1000)?;
                }
            }
        }
        let mut context = origin.clone();
        context.generation = context
            .generation
            .parse::<i64>()
            .map_err(|_| invalid("invalid generation"))?
            .checked_add(1)
            .ok_or_else(|| invalid("generation exhausted"))?
            .to_string();
        let result = DataChangeResult {
            request_id: id.into(),
            phase: DataPhase::Committed,
            context: context.clone(),
            before_generation: origin.generation.clone(),
            after_generation: context.generation.clone(),
            snapshot_id: snapshot,
            created,
            changed,
            conflicts: vec![],
            reason: None,
            resolved_by_request: None,
        };
        tx.execute(
            "UPDATE _pc_meta SET value=?1 WHERE key='context'",
            [serde_json::to_string(&context)?],
        )?;
        tx.execute(
            "INSERT INTO _pc_receipts VALUES(?1,?2)",
            params![
                id,
                serde_json::to_string(&Receipt {
                    principal: principal.into(),
                    digest: payload.into(),
                    origin: origin.clone(),
                    result: result.clone()
                })?
            ],
        )?;
        self.verify_commit()?;
        tx.commit()?;
        Ok(result)
    }
    fn verify_commit(&self) -> Result<()> {
        if let Some(check) = &self.before_commit {
            check()?;
        }
        Ok(())
    }
    pub fn query(
        &self,
        s: &DataSql,
        limit: u32,
    ) -> Result<(Vec<String>, Vec<Vec<DataCell>>, bool)> {
        run_sql(&self.conn, s, true, false, limit)
    }
    pub fn preview(&mut self, p: DataPreviewParams, principal: &str) -> Result<DataPreview> {
        self.check(&p.context)?;
        if p.statements.is_empty() || p.statements.len() > 32 {
            return Err(invalid("bounded SQL batch required"));
        }
        let tx = self.conn.unchecked_transaction()?;
        let schema: i64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        let before_tables = business_counts(&tx)?;
        let before = tx.total_changes();
        let deadline = Instant::now() + Duration::from_secs(5);
        for s in &p.statements {
            if Instant::now() >= deadline {
                return Err(invalid("preview deadline exceeded"));
            }
            run_sql(&tx, s, false, true, 1000)?;
        }
        let after_tables = business_counts(&tx)?;
        let dropped_rows: u64 = before_tables
            .iter()
            .filter(|(name, _)| !after_tables.contains_key(*name))
            .map(|(_, count)| *count)
            .sum();
        let affected = (tx.total_changes() - before).saturating_add(dropped_rows);
        let changed = tx.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))? != schema;
        tx.rollback()?;
        let preview = DataPreview {
            context: p.context,
            token: uuid::Uuid::new_v4().to_string(),
            statements: p.statements,
            affected_rows: affected,
            schema_changed: changed,
        };
        self.verify_commit()?;
        self.conn.execute(
            "INSERT INTO _pc_previews VALUES(?1,?2)",
            params![
                preview.token,
                serde_json::to_string(&PreviewRecord {
                    principal: principal.into(),
                    preview: preview.clone()
                })?
            ],
        )?;
        Ok(preview)
    }
    pub fn preview_get(&self, p: DataPreviewGetParams) -> Result<DataPreview> {
        self.check(&p.context)?;
        let raw: String = self.conn.query_row(
            "SELECT body FROM _pc_previews WHERE id=?1",
            [p.token],
            |r| r.get(0),
        )?;
        let record: PreviewRecord = serde_json::from_str(&raw)?;
        if record.preview.context != p.context {
            return Err(invalid("preview context changed"));
        }
        Ok(record.preview)
    }
    pub fn confirm(&mut self, p: DataConfirmParams, principal: &str) -> Result<DataChangeResult> {
        let payload = digest(&p)?;
        if let Some(r) = self.existing(&p.request_id, &p.context, principal, &payload)? {
            return Ok(r);
        }
        self.check(&p.context)?;
        let raw: String = self.conn.query_row(
            "SELECT body FROM _pc_previews WHERE id=?1",
            [&p.token],
            |r| r.get(0),
        )?;
        let record: PreviewRecord = serde_json::from_str(&raw)?;
        // Agent may prepare a preview, but confirmation itself is admitted only by
        // the authenticated Human runtime. Token + exact generation binds effects.
        if record.preview.context != p.context {
            return Err(invalid("preview generation changed"));
        }
        let snapshot = self.snapshot()?;
        let mutations = record
            .preview
            .statements
            .into_iter()
            .map(|s| Mutation::Sql {
                sql: s.sql,
                params: s.params,
            })
            .collect();
        self.apply(
            &p.request_id,
            &p.context,
            principal,
            &payload,
            BTreeMap::new(),
            mutations,
            true,
            Some(snapshot),
        )
    }
    fn snapshots_dir(&self) -> Result<PathBuf> {
        let p = self
            .path
            .parent()
            .unwrap()
            .join(format!("{}-snapshots", self.base.namespace));
        private_dir(&p)?;
        Ok(p)
    }
    pub fn snapshot(&self) -> Result<String> {
        self.verify_commit()?;
        let dir = self.snapshots_dir()?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = dir.join(format!("{id}.sqlite"));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        self.conn.backup("main", &path, None)?;
        let record = DataSnapshot {
            id: id.clone(),
            created_at: crate::now_ts(),
            generation: self.context()?.generation,
        };
        self.conn.execute(
            "INSERT INTO _pc_snapshots VALUES(?1,?2)",
            params![id, serde_json::to_string(&record)?],
        )?;
        Ok(id)
    }
    pub fn snapshots(&self) -> Result<Vec<DataSnapshot>> {
        let mut q = self
            .conn
            .prepare("SELECT body FROM _pc_snapshots ORDER BY id LIMIT 1000")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn restore(&mut self, p: DataRestoreParams, principal: &str) -> Result<DataChangeResult> {
        let payload = digest(&p)?;
        if let Some(r) = self.existing(&p.request_id, &p.context, principal, &payload)? {
            return Ok(r);
        }
        self.check(&p.context)?;
        valid_uuid(&p.snapshot_id)?;
        if !self.snapshots()?.iter().any(|s| s.id == p.snapshot_id) {
            return Err(invalid("unknown recovery snapshot"));
        }
        let path = self
            .snapshots_dir()?
            .join(format!("{}.sqlite", p.snapshot_id));
        private_file(&path)?;
        let source =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let ok: String = source.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(invalid("recovery snapshot failed integrity check"));
        }
        let raw: String =
            source.query_row("SELECT value FROM _pc_meta WHERE key='context'", [], |r| {
                r.get(0)
            })?;
        let source_context: DataContext = serde_json::from_str(&raw)?;
        if source_context.namespace != p.context.namespace
            || source_context.db_id != p.context.db_id
            || source_context.authority_id != p.context.authority_id
        {
            return Err(invalid("snapshot authority differs"));
        }
        let safety = self.snapshot()?;
        // Build a private stage: business data rewinds, immutable request receipts,
        // current object revisions and recovery records survive. Core/ACL untouched.
        let stage = self
            .snapshots_dir()?
            .join(format!("{}.stage", uuid::Uuid::new_v4()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&stage)?;
        source.backup("main", &stage, None)?;
        let mut staged = Connection::open(&stage)?;
        let tx = staged.transaction()?;
        for table in ["_pc_receipts", "_pc_revisions", "_pc_snapshots"] {
            let mut q = self.conn.prepare(&format!("SELECT * FROM {table}"))?;
            let cols = q.column_count();
            let rows = q.query_map([], |r| {
                (0..cols)
                    .map(|i| r.get::<_, SqlValue>(i))
                    .collect::<std::result::Result<Vec<_>, _>>()
            })?;
            for row in rows {
                let row = row?;
                let marks = (1..=cols)
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(",");
                tx.execute(
                    &format!("INSERT OR REPLACE INTO {table} VALUES({marks})"),
                    rusqlite::params_from_iter(row),
                )?;
            }
        }
        // Revision counters cannot rewind and collide with retained histories.
        let latest = {
            let mut q =
                tx.prepare("SELECT kind,id,MAX(revision) FROM _pc_revisions GROUP BY kind,id")?;
            q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let restored_revisions: BTreeMap<_, _> = latest
            .iter()
            .map(|(kind, id, max)| {
                Ok((
                    (kind.clone(), id.clone()),
                    max.checked_add(1)
                        .ok_or_else(|| invalid("revision exhausted"))?
                        .to_string(),
                ))
            })
            .collect::<Result<_>>()?;
        for (kind, id, max) in latest {
            let old: Option<String> = tx
                .query_row(
                    "SELECT body FROM _pc_objects WHERE kind=?1 AND id=?2",
                    params![kind, id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(old) = old else { continue };
            let mut stored: Stored = serde_json::from_str(&old)?;
            let next = max
                .checked_add(1)
                .ok_or_else(|| invalid("revision exhausted"))?;
            match &mut stored.object {
                DataObject::Block {
                    revision,
                    provenance,
                    ..
                }
                | DataObject::View {
                    revision,
                    provenance,
                    ..
                }
                | DataObject::Relation {
                    revision,
                    provenance,
                    ..
                } => {
                    *revision = next.to_string();
                    provenance.updated_by = crewd_proto::ResourceActor {
                        kind: "human".into(),
                        session_id: None,
                    };
                    provenance.updated_at = crate::now_ts();
                }
            };
            for capture in &mut stored.captures {
                if capture.live && capture.namespace == p.context.namespace
                    && let Some(revision) =
                        restored_revisions.get(&(capture.kind.clone(), capture.id.clone()))
                {
                    capture.revision = revision.clone();
                }
            }
            let raw = serde_json::to_string(&stored)?;
            tx.execute(
                "UPDATE _pc_objects SET revision=?3,body=?4 WHERE kind=?1 AND id=?2",
                params![kind, id, next, raw],
            )?;
            tx.execute(
                "INSERT INTO _pc_revisions VALUES(?1,?2,?3,?4)",
                params![kind, id, next, raw],
            )?;
        }
        let mut context = p.context.clone();
        context.restore_epoch = uuid::Uuid::new_v4().to_string();
        context.generation = context
            .generation
            .parse::<i64>()
            .map_err(|_| invalid("invalid generation"))?
            .checked_add(1)
            .ok_or_else(|| invalid("generation exhausted"))?
            .to_string();
        let result = DataChangeResult {
            request_id: p.request_id.clone(),
            phase: DataPhase::Committed,
            context: context.clone(),
            before_generation: p.context.generation.clone(),
            after_generation: context.generation.clone(),
            snapshot_id: Some(safety),
            created: BTreeMap::new(),
            changed: vec![],
            conflicts: vec![],
            reason: Some(
                "Knowledge restored; current Core ACL and immutable request history retained"
                    .into(),
            ),
            resolved_by_request: None,
        };
        tx.execute(
            "UPDATE _pc_meta SET value=?1 WHERE key='context'",
            [serde_json::to_string(&context)?],
        )?;
        tx.execute("DELETE FROM _pc_previews", [])?;
        tx.execute(
            "INSERT INTO _pc_receipts VALUES(?1,?2)",
            params![
                p.request_id,
                serde_json::to_string(&Receipt {
                    principal: principal.into(),
                    digest: payload,
                    origin: p.context,
                    result: result.clone()
                })?
            ],
        )?;
        tx.commit()?;
        self.verify_commit()?;
        {
            let guard = &self.before_commit;
            let backup = rusqlite::backup::Backup::new(&staged, &mut self.conn)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(check) = guard {
                    check()?;
                }
                if Instant::now() >= deadline {
                    return Err(invalid(
                        "recovery copy deadline exceeded; old state retained",
                    ));
                }
                match backup.step(128)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {}
                    rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                        return Err(invalid("recovery copy busy; old state retained"));
                    }
                    _ => return Err(invalid("recovery copy state unknown")),
                }
            }
        }
        drop(staged);
        std::fs::remove_file(stage)?;
        Ok(result)
    }
}
fn business_counts(c: &Connection) -> Result<BTreeMap<String, u64>> {
    let names = {
        let mut q = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE '_pc_%' AND name NOT LIKE 'sqlite_%' LIMIT 257")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    if names.len() > 256 {
        return Err(invalid("schema too large for a safe impact preview"));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    c.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
    let result = (|| {
        let mut counts = BTreeMap::new();
        for name in names {
            let escaped = name.replace('"', "\"\"");
            let count: i64 =
                c.query_row(&format!("SELECT count(*) FROM \"{escaped}\""), [], |r| {
                    r.get(0)
                })?;
            counts.insert(
                name,
                u64::try_from(count).map_err(|_| invalid("invalid row count"))?,
            );
        }
        Ok(counts)
    })();
    c.progress_handler(0, None::<fn() -> bool>)?;
    result
}
pub fn bound(limit: u32, max: u32) -> Result<()> {
    if limit == 0 || limit > max {
        return Err(invalid("page limit out of bounds"));
    }
    Ok(())
}
fn valid_uuid(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(invalid("original UUID required"));
    }
    Ok(())
}
fn value(c: &DataCell) -> Result<SqlValue> {
    Ok(match c {
        DataCell::Null => SqlValue::Null,
        DataCell::Integer(s) => SqlValue::Integer(s.parse().map_err(|_| invalid("invalid int64"))?),
        DataCell::Real(v) if v.is_finite() => SqlValue::Real(*v),
        DataCell::Real(_) => return Err(invalid("nonfinite real")),
        DataCell::Text(s) => SqlValue::Text(s.clone()),
        DataCell::BlobHex(s) => {
            if !s.is_ascii() || s.len() % 2 != 0 {
                return Err(invalid("invalid blob hex"));
            }
            SqlValue::Blob(
                (0..s.len())
                    .step_by(2)
                    .map(|i| {
                        u8::from_str_radix(&s[i..i + 2], 16)
                            .map_err(|_| invalid("invalid blob hex"))
                    })
                    .collect::<Result<Vec<_>>>()?,
            )
        }
    })
}
fn cell(v: ValueRef<'_>) -> Result<DataCell> {
    Ok(match v {
        ValueRef::Null => DataCell::Null,
        ValueRef::Integer(i) => DataCell::Integer(i.to_string()),
        ValueRef::Real(v) if v.is_finite() => DataCell::Real(v),
        ValueRef::Real(_) => return Err(invalid("nonfinite real")),
        ValueRef::Text(s) => DataCell::Text(
            std::str::from_utf8(s)
                .map_err(|_| invalid("non UTF-8 text"))?
                .into(),
        ),
        ValueRef::Blob(s) => DataCell::BlobHex(s.iter().map(|b| format!("{b:02x}")).collect()),
    })
}
fn protected(name: &str) -> bool {
    let s = name.to_ascii_lowercase();
    s.starts_with("_pc_") || s.starts_with("sqlite_") || s.starts_with("pragma_")
}
/// Default deny includes indirect trigger/view reads. Statement preparation happens
/// under the authorizer and explicit transaction commands are never permitted.
fn authorize(
    ctx: AuthContext<'_>,
    readonly: bool,
    confirmed: bool,
    danger: &AtomicBool,
) -> Authorization {
    use AuthAction::*;
    if ctx.database_name.is_some_and(|s| s != "main") {
        return Authorization::Deny;
    }
    let ok = match ctx.action {
        Select | Recursive => true,
        Read { table_name, .. } => {
            !protected(table_name)
                || matches!(
                    table_name,
                    "sqlite_master" | "sqlite_schema" | "sqlite_sequence"
                )
        }
        Insert { table_name } => !readonly && !table_name.to_ascii_lowercase().starts_with("_pc_"),
        Update { table_name, .. } | Delete { table_name } => {
            if table_name == "sqlite_master" || table_name == "sqlite_schema" {
                !readonly
            } else if protected(table_name) || readonly {
                false
            } else if confirmed {
                true
            } else {
                danger.store(true, Ordering::Relaxed);
                false
            }
        }
        CreateTable { table_name } | CreateIndex { table_name, .. } => {
            !readonly && !protected(table_name)
        }
        DropTable { table_name } | DropIndex { table_name, .. } => {
            if protected(table_name) || readonly {
                false
            } else if confirmed {
                true
            } else {
                danger.store(true, Ordering::Relaxed);
                false
            }
        }
        Function { function_name } => matches!(
            function_name.to_ascii_lowercase().as_str(),
            "abs"
                | "coalesce"
                | "ifnull"
                | "nullif"
                | "min"
                | "max"
                | "sum"
                | "total"
                | "avg"
                | "count"
                | "length"
                | "lower"
                | "upper"
                | "trim"
                | "ltrim"
                | "rtrim"
                | "substr"
                | "substring"
                | "replace"
                | "round"
                | "typeof"
                | "quote"
                | "hex"
                | "unicode"
                | "char"
                | "instr"
                | "like"
                | "glob"
                | "json"
                | "json_extract"
                | "json_array"
                | "json_object"
                | "json_type"
                | "json_valid"
                | "date"
                | "time"
                | "datetime"
                | "strftime"
        ),
        _ => false,
    };
    if ok {
        Authorization::Allow
    } else {
        Authorization::Deny
    }
}
fn run_sql(
    c: &Connection,
    s: &DataSql,
    readonly: bool,
    confirmed: bool,
    limit: u32,
) -> Result<(Vec<String>, Vec<Vec<DataCell>>, bool)> {
    bound(limit, 1000)?;
    if s.sql.len() > 65536 || s.params.len() > 256 {
        return Err(invalid("SQL input bounds exceeded"));
    }
    // SQLite REPLACE can delete conflicting rows without an UPDATE authorizer
    // event. Be conservative rather than infer safety from the keyword shape.
    if !confirmed && !readonly && s.sql.to_ascii_lowercase().contains("replace") {
        return Err(invalid(
            "replacement SQL requires Human preview confirmation",
        ));
    }
    let danger = Arc::new(AtomicBool::new(false));
    let d = danger.clone();
    c.authorizer(Some(move |ctx: AuthContext<'_>| {
        authorize(ctx, readonly, confirmed, &d)
    }))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    c.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
    let result = (|| {
        let mut stmt = c.prepare(&s.sql)?;
        if readonly && !stmt.readonly() {
            return Err(invalid("query must be readonly"));
        }
        let values = s.params.iter().map(value).collect::<Result<Vec<_>>>()?;
        let columns = stmt
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut q = stmt.query(rusqlite::params_from_iter(values))?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        while let Some(row) = q.next()? {
            if rows.len() == limit as usize {
                truncated = true;
                break;
            }
            let cells = (0..columns.len())
                .map(|i| cell(row.get_ref(i)?))
                .collect::<Result<Vec<_>>>()?;
            bytes += serde_json::to_vec(&cells)?.len();
            if bytes > 1024 * 1024 {
                return Err(invalid("SQL result byte bound exceeded"));
            }
            rows.push(cells);
        }
        Ok((columns, rows, truncated))
    })();
    c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
    c.progress_handler(0, None::<fn() -> bool>)?;
    if danger.load(Ordering::Relaxed) {
        return Err(invalid(
            "destructive SQL requires data.preview and real Human data.confirm",
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn base() -> DataContext {
        DataContext {
            db_id: "test-core".into(),
            authority_id: "test-core".into(),
            restore_epoch: "initial".into(),
            crew_id: "crew".into(),
            namespace: namespace("test-core", "crew"),
            generation: "0".into(),
        }
    }
    fn sql(sql: &str) -> Mutation {
        Mutation::Sql {
            sql: sql.into(),
            params: vec![],
        }
    }
    fn apply(k: &mut Knowledge, mutations: Vec<Mutation>) -> DataChangeResult {
        let c = k.context().unwrap();
        k.apply(
            &uuid::Uuid::new_v4().to_string(),
            &c,
            "session:alice",
            "payload",
            BTreeMap::new(),
            mutations,
            false,
            None,
        )
        .unwrap()
    }
    fn object(id: &str, revision: &str, content: &str) -> DataObject {
        DataObject::Block {
            id: id.into(),
            revision: revision.into(),
            content_type: "text/plain".into(),
            content: Value::String(content.into()),
            archived: false,
            provenance: DataProvenance {
                created_by: crewd_proto::ResourceActor {
                    kind: "human".into(),
                    session_id: None,
                },
                updated_by: crewd_proto::ResourceActor {
                    kind: "human".into(),
                    session_id: None,
                },
                created_at: "now".into(),
                updated_at: "now".into(),
                sources: vec![],
                attributes: Value::Null,
            },
        }
    }
    #[test]
    fn sqlite_durable_uuid_cas_history_and_owner_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path())
            .unwrap()
            .join("private/knowledge.sqlite");
        let c = base();
        let mut k = Knowledge::open(&path, c.clone(), true).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        let r = DataRef {
            namespace: c.namespace.clone(),
            kind: "block".into(),
            id: id.clone(),
        };
        let mutations = vec![Mutation::PutObject {
            object: object(&id, "1", "original"),
            expected_revision: None,
            captures: vec![],
        }];
        k.apply(
            &request,
            &c,
            "session:alice",
            "digest",
            BTreeMap::new(),
            mutations,
            false,
            None,
        )
        .unwrap();
        drop(k);
        let mut k = Knowledge::open(&path, c.clone(), false).unwrap();
        assert_eq!(k.context().unwrap().generation, "1");
        assert!(
            k.retry(&request, &c, "session:alice", "digest")
                .unwrap()
                .is_some()
        );
        assert!(k.retry(&request, &c, "session:bob", "digest").is_err());
        assert!(k.retry(&request, &c, "session:alice", "different").is_err());
        assert!(k.lookup(&request, &c, "session:bob", false).is_err());
        assert!(matches!(
            k.lookup(&request, &c, "human-admin", true).unwrap(),
            DataRequestLookup::Known(_)
        ));
        apply(
            &mut k,
            vec![Mutation::PutObject {
                object: object(&id, "2", "new"),
                expected_revision: Some("1".into()),
                captures: vec![],
            }],
        );
        assert_eq!(object_fields(&k.get(&r, Some("1")).unwrap().object).2, "1");
        assert_eq!(k.history(&r, None, 10).unwrap().0.len(), 2);
        assert!(
            k.apply(
                &uuid::Uuid::new_v4().to_string(),
                &c,
                "session:alice",
                "p",
                BTreeMap::new(),
                vec![],
                false,
                None
            )
            .is_err()
        );
    }
    #[test]
    fn sqlite_real_authorizer_transaction_rollback_and_dangerous_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let mut k = Knowledge::open(
            &std::fs::canonicalize(dir.path())
                .unwrap()
                .join("private/knowledge.sqlite"),
            base(),
            true,
        )
        .unwrap();
        apply(
            &mut k,
            vec![
                sql("CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT)"),
                sql("INSERT INTO items VALUES(1,'one'),(2,'two')"),
            ],
        );
        for statement in [
            "SELECT * FROM _pc_meta",
            "ATTACH ':memory:' AS other",
            "PRAGMA writable_schema=ON",
            "SELECT load_extension('/tmp/no')",
            "SELECT readfile('/tmp/no')",
            "UPDATE _pc_receipts SET body='bad'",
            "CREATE TRIGGER escape AFTER INSERT ON items BEGIN DELETE FROM _pc_objects; END",
            "DROP TABLE items",
            "UPDATE items SET value='overwrite'",
            "DELETE FROM items",
        ] {
            let c = k.context().unwrap();
            let result = k.apply(
                &uuid::Uuid::new_v4().to_string(),
                &c,
                "session:alice",
                "bad",
                BTreeMap::new(),
                vec![sql(statement)],
                false,
                None,
            );
            assert!(result.is_err(), "must refuse {statement}");
            assert_eq!(k.context().unwrap(), c);
        }
        let c = k.context().unwrap();
        assert!(
            k.apply(
                &uuid::Uuid::new_v4().to_string(),
                &c,
                "session:alice",
                "rollback",
                BTreeMap::new(),
                vec![
                    sql("INSERT INTO items VALUES(3,'three')"),
                    sql("SELECT * FROM _pc_meta")
                ],
                false,
                None
            )
            .is_err()
        );
        assert_eq!(
            k.conn
                .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let p = k
            .preview(
                DataPreviewParams {
                    context: c.clone(),
                    statements: vec![DataSql {
                        sql: "DROP TABLE items".into(),
                        params: vec![],
                    }],
                },
                "session:alice",
            )
            .unwrap();
        assert!(p.schema_changed);
        assert_eq!(
            p.affected_rows, 2,
            "DROP preview includes rows lost with schema"
        );
        assert_eq!(
            k.conn
                .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let confirmed = k
            .confirm(
                DataConfirmParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    context: c,
                    token: p.token,
                },
                "human:current",
            )
            .unwrap();
        let snapshot = confirmed.snapshot_id.unwrap();
        assert!(k.conn.prepare("SELECT * FROM items").is_err());
        let before = k.context().unwrap();
        let restore_id = uuid::Uuid::new_v4().to_string();
        k.restore(
            DataRestoreParams {
                request_id: restore_id.clone(),
                context: before.clone(),
                snapshot_id: snapshot,
            },
            "human:current",
        )
        .unwrap();
        assert_ne!(k.context().unwrap().restore_epoch, before.restore_epoch);
        assert_eq!(
            k.conn
                .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(matches!(
            k.lookup(&restore_id, &before, "human:current", true)
                .unwrap(),
            DataRequestLookup::Historical(_)
        ));
    }
    #[test]
    fn sqlite_closed_scope_rolls_back_and_foreign_format_is_not_initialized() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path())
            .unwrap()
            .join("private/knowledge.sqlite");
        let mut k = Knowledge::open(&path, base(), true).unwrap();
        let context = k.context().unwrap();
        k.before_commit = Some(Box::new(|| Err(invalid("original scope closed"))));
        assert!(
            k.apply(
                &uuid::Uuid::new_v4().to_string(),
                &context,
                "session:alice",
                "p",
                BTreeMap::new(),
                vec![sql("CREATE TABLE refused(id INTEGER)")],
                false,
                None
            )
            .is_err()
        );
        assert_eq!(k.context().unwrap(), context);
        assert!(k.conn.prepare("SELECT * FROM refused").is_err());
        let foreign = path.parent().unwrap().join("foreign.sqlite");
        let c = Connection::open(&foreign).unwrap();
        c.execute_batch("CREATE TABLE retained(x);").unwrap();
        drop(c);
        std::fs::set_permissions(&foreign, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Knowledge::open(&foreign, base(), true).is_err());
        let c = Connection::open(&foreign).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='_pc_meta'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
