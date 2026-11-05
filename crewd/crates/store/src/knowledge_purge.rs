//! Two-store crew purge recovery. All prepare/settle work runs on the Core FIFO
//! (or before that FIFO opens), under the same data-directory lease as knowledge.
use crate::{Result, Store, StoreError, backup, fsutil, meta_get, meta_set, team_data};
use crewd_proto::team_data::DataContext;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const PENDING: &str = ".crew-purge-pending-v1";
const RETAINED: &str = "crew-purge-v1";
const MANIFEST: &str = "manifest.json";
const PLAN: &str = "plan.json";
const COMPLETED: &str = "completed.json";
const SAFETY: &str = "safety";
const LIVE: &str = "live";
const HISTORY: &str = "history";
const CORE: &str = "core.db";
const KNOWLEDGE: &str = "knowledge.sqlite";
const MAX_HISTORY_FILES: usize = 1024;
const MAX_PENDING: usize = 64;
const MAX_MANIFEST: u64 = 512 * 1024;

fn invalid(detail: &str) -> StoreError {
    StoreError::Invalid(format!("crew knowledge purge: {detail}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    format: u32,
    operation: String,
    database: String,
    restore_stamp: String,
    crew: String,
    namespace: String,
    started_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Database,
    Wal,
    Shm,
    Journal,
    History,
}
impl Kind {
    const ALL: [Self; 5] = [
        Self::Database,
        Self::Wal,
        Self::Shm,
        Self::Journal,
        Self::History,
    ];
    fn name(self, ns: &str) -> String {
        match self {
            Self::Database => format!("{ns}.sqlite"),
            Self::Wal => format!("{ns}.sqlite-wal"),
            Self::Shm => format!("{ns}.sqlite-shm"),
            Self::Journal => format!("{ns}.sqlite-journal"),
            Self::History => format!("{ns}-snapshots"),
        }
    }
    fn directory(self) -> bool {
        self == Self::History
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    kind: Kind,
    dev: u64,
    ino: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileProof {
    name: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    origin: Origin,
    originals: Vec<Original>,
    files: Vec<FileProof>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Committed {
    plan_sha256: String,
    committed_at_ms: i64,
}

pub struct Prepared {
    operation: PathBuf,
    knowledge: PathBuf,
    retained: PathBuf,
    plan: Plan,
}

fn metadata(path: &Path, directory: bool) -> Result<Option<Metadata>> {
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        }
    {
        return Err(invalid("expected an owned private non-aliased artifact"));
    }
    Ok(Some(m))
}

fn directory(path: &Path) -> Result<()> {
    if metadata(path, true)?.is_none() {
        fs::DirBuilder::new().mode(0o700).create(path)?;
        fsutil::fsync_dir(
            path.parent()
                .ok_or_else(|| invalid("directory parent missing"))?,
        )?;
    }
    Ok(())
}

fn bounded_read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let m = metadata(path, false)?.ok_or_else(|| invalid("required manifest missing"))?;
    if m.len() > MAX_MANIFEST {
        return Err(invalid("manifest exceeds bound"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(invalid("manifest exceeds bound"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_record(path: &Path, record: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(invalid("manifest exceeds bound"));
    }
    fsutil::atomic_write(path, &bytes, 0o600)?;
    Ok(())
}

fn uuid_name(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s)
        .ok()
        .filter(|u| u.to_string() == s)
        .ok_or_else(|| invalid("unexpected operation or snapshot identity"))
}

fn history_name(s: &str) -> Result<()> {
    let id = s
        .strip_suffix(".sqlite")
        .or_else(|| s.strip_suffix(".sqlite-wal"))
        .or_else(|| s.strip_suffix(".sqlite-shm"))
        .or_else(|| s.strip_suffix(".stage"))
        .ok_or_else(|| invalid("unexpected recovery snapshot filename"))?;
    uuid_name(id)?;
    Ok(())
}

fn validate_history(
    root: &Path,
    names: &[String],
    registered: Option<&BTreeSet<String>>,
) -> Result<()> {
    let contains = |name: &str| {
        names
            .binary_search_by(|item| item.as_str().cmp(name))
            .is_ok()
    };
    for name in names {
        history_name(name)?;
        metadata(&root.join(name), false)?
            .ok_or_else(|| invalid("recovery snapshot is missing"))?;
        if let Some(base) = name.strip_suffix("-wal") {
            if !contains(base)
                || registered.is_some_and(|known| !known.contains(base))
                || !team_data::legacy_snapshot_sidecars(&root.join(base))?
            {
                return Err(invalid("unexpected legacy recovery snapshot sidecars"));
            }
        } else if let Some(base) = name.strip_suffix("-shm")
            && !contains(&format!("{base}-wal"))
        {
            return Err(invalid("unpaired recovery snapshot index"));
        }
    }
    Ok(())
}

fn names(dir: &Path, limit: usize) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for item in fs::read_dir(dir)? {
        if names.len() >= limit {
            return Err(invalid("artifact count exceeds bound"));
        }
        names.push(
            item?
                .file_name()
                .into_string()
                .map_err(|_| invalid("unexpected filename encoding"))?,
        );
    }
    names.sort();
    Ok(names)
}

fn crew_exists(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM crews WHERE id=?1)",
        [id],
        |r| r.get(0),
    )?)
}

fn valid_origin(o: &Origin, database: &str, restore_stamp: &str, op: Uuid) -> Result<()> {
    if o.format != 1
        || o.database != database
        || o.restore_stamp != restore_stamp
        || uuid_name(&o.restore_stamp).is_err()
        || o.operation != op.to_string()
        || o.crew.is_empty()
        || o.crew.len() > 256
        || o.crew.chars().any(char::is_control)
        || o.namespace != team_data::namespace(database, &o.crew)
        || o.started_at_ms <= 0
    {
        return Err(invalid("original purge scope differs"));
    }
    Ok(())
}

fn scan(knowledge: &Path, ns: &str) -> Result<Vec<Original>> {
    if metadata(knowledge, true)?.is_none() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for kind in Kind::ALL {
        if let Some(m) = metadata(&knowledge.join(kind.name(ns)), kind.directory())? {
            out.push(Original {
                kind,
                dev: m.dev(),
                ino: m.ino(),
            });
        }
    }
    if !out.is_empty() && out[0].kind != Kind::Database {
        return Err(invalid(
            "namespace database is missing but owned recovery artifacts remain",
        ));
    }
    Ok(out)
}

fn verify_context(c: &Connection, o: &Origin) -> Result<()> {
    let application: i64 = c.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if application != 0x50434b31 || version != 1 {
        return Err(invalid("unsupported knowledge SQLite format"));
    }
    let raw: String = c.query_row("SELECT value FROM _pc_meta WHERE key='context'", [], |r| {
        r.get(0)
    })?;
    let scope: DataContext = serde_json::from_str(&raw)?;
    if scope.db_id != o.database
        || scope.authority_id != o.database
        || scope.crew_id != o.crew
        || scope.namespace != o.namespace
    {
        return Err(invalid("knowledge authority differs"));
    }
    Ok(())
}

fn read_db(path: &Path) -> Result<Connection> {
    metadata(path, false)?.ok_or_else(|| invalid("required SQLite file missing"))?;
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

fn read_snapshot(path: &Path) -> Result<Connection> {
    metadata(path, false)?.ok_or_else(|| invalid("required SQLite file missing"))?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        if metadata(Path::new(&sidecar), false)?.is_some() {
            return Err(invalid("standalone snapshot has a SQLite sidecar"));
        }
    }
    Ok(Connection::open_with_flags(
        crate::file_uri(path, "immutable=1"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?)
}

fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    metadata(src, false)?.ok_or_else(|| invalid("recovery snapshot is missing"))?;
    let mut input = File::open(src)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dst)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}

fn proof(root: &Path, name: String) -> Result<FileProof> {
    let path = root.join(&name);
    let m = metadata(&path, false)?.ok_or_else(|| invalid("safety pair file is missing"))?;
    let mut input = File::open(&path)?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        bytes += n as u64;
    }
    if bytes != m.len() {
        return Err(invalid("safety pair changed during verification"));
    }
    let sha256 = hash
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(FileProof {
        name,
        sha256,
        bytes,
    })
}

fn verify_safety(root: &Path, plan: &Plan, committed: Option<&Committed>) -> Result<()> {
    if metadata(root, true)?.is_none()
        || plan.files.len() < 2
        || plan.files.len() > MAX_HISTORY_FILES + 2
        || plan.files[0].name != CORE
        || plan.files[1].name != KNOWLEDGE
    {
        return Err(invalid("incomplete paired safety snapshot"));
    }
    let old = plan
        .files
        .iter()
        .skip(2)
        .map(|f| {
            let name = f
                .name
                .strip_prefix("history/")
                .ok_or_else(|| invalid("unexpected safety filename"))?;
            history_name(name)?;
            Ok(name.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    if old.windows(2).any(|w| w[0] >= w[1]) {
        return Err(invalid("noncanonical safety manifest"));
    }
    let history = root.join(HISTORY);
    if metadata(&history, true)?.is_none() || names(&history, MAX_HISTORY_FILES)? != old {
        return Err(invalid("safety history membership differs"));
    }
    validate_history(&history, &old, None)?;
    let mut expected = vec![CORE, HISTORY, KNOWLEDGE, MANIFEST];
    let complete = root.join(COMPLETED);
    if metadata(&complete, false)?.is_some() {
        let found: Committed = bounded_read(&complete)?;
        if committed != Some(&found) {
            return Err(invalid("completion proof differs"));
        }
        expected.insert(0, COMPLETED);
    }
    if names(root, expected.len())? != expected {
        return Err(invalid("unexpected safety pair contents"));
    }
    for file in &plan.files {
        if proof(root, file.name.clone())? != *file {
            return Err(invalid("safety pair checksum differs"));
        }
    }
    if bounded_read::<Plan>(&root.join(MANIFEST))? != *plan {
        return Err(invalid("safety manifest changed"));
    }
    Ok(())
}

fn verify_originals(plan: &Plan) -> Result<()> {
    let kinds: Vec<_> = plan.originals.iter().map(|p| p.kind).collect();
    let expected: Vec<_> = Kind::ALL
        .into_iter()
        .filter(|k| kinds.contains(k))
        .collect();
    if kinds.first() != Some(&Kind::Database) || kinds != expected {
        return Err(invalid("invalid original artifact manifest"));
    }
    Ok(())
}

/// Returns None only when no exact artifact exists for this crew.
pub fn prepare(store: &Store, data: &Path, crew: &str) -> Result<Option<Prepared>> {
    let knowledge = data.join("knowledge");
    let db = meta_get(store.conn(), "dbId")?.ok_or_else(|| invalid("Core identity missing"))?;
    let ns = team_data::namespace(&db, crew);
    let original = scan(&knowledge, &ns)?;
    if original.is_empty() {
        return Ok(None);
    }
    if store.backups_dir().is_none() {
        return Err(invalid("backup directory required"));
    }
    let backups = data.join(backup::BACKUPS_DIR);
    directory(&backups)?;
    let retained = backups.join(RETAINED);
    directory(&retained)?;
    let pending = data.join(PENDING);
    directory(&pending)?;
    let o = Origin {
        format: 1,
        operation: Uuid::new_v4().to_string(),
        database: db,
        restore_stamp: crate::personal_drafts::epoch(store.conn())?,
        crew: crew.into(),
        namespace: ns,
        started_at_ms: crate::now_ms(),
    };
    valid_origin(&o, &o.database, &o.restore_stamp, uuid_name(&o.operation)?)?;
    let operation = pending.join(&o.operation);
    directory(&operation)?;
    write_record(&operation.join(MANIFEST), &o)?;

    // A complete, verified, private safety pair exists before anything is moved.
    let safety = operation.join(SAFETY);
    directory(&safety)?;
    let history = safety.join(HISTORY);
    directory(&history)?;
    let live = operation.join(LIVE);
    directory(&live)?;
    backup::vacuum_into(store.conn(), &safety.join(CORE))?;
    let mut files = vec![proof(&safety, CORE.into())?];
    {
        let source_path = knowledge.join(Kind::Database.name(&o.namespace));
        let source = if original
            .iter()
            .any(|file| matches!(file.kind, Kind::Wal | Kind::Shm | Kind::Journal))
        {
            read_db(&source_path)?
        } else {
            read_snapshot(&source_path)?
        };
        verify_context(&source, &o)?;
        backup::vacuum_into(&source, &safety.join(KNOWLEDGE))?;
        let copied = read_snapshot(&safety.join(KNOWLEDGE))?;
        verify_context(&copied, &o)?;
        files.push(proof(&safety, KNOWLEDGE.into())?);
        let existing = knowledge.join(Kind::History.name(&o.namespace));
        let old = if metadata(&existing, true)?.is_some() {
            names(&existing, MAX_HISTORY_FILES)?
        } else {
            Vec::new()
        };
        let registered = {
            let mut q = source.prepare("SELECT id FROM _pc_snapshots ORDER BY id LIMIT ?1")?;
            let rows = q.query_map([MAX_HISTORY_FILES as i64 + 1], |r| r.get::<_, String>(0))?;
            let mut registered = BTreeSet::new();
            for id in rows {
                let id = id?;
                uuid_name(&id)?;
                let name = format!("{id}.sqlite");
                if old.binary_search(&name).is_err() {
                    return Err(invalid("registered recovery snapshot is missing"));
                }
                registered.insert(name);
            }
            registered
        };
        validate_history(&existing, &old, Some(&registered))?;
        for name in old {
            let src = existing.join(&name);
            let dst = history.join(&name);
            copy_file(&src, &dst)?;
            if !name.ends_with("-wal") && !name.ends_with("-shm") {
                let copied = read_snapshot(&dst)?;
                verify_context(&copied, &o)?;
                if copied.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? != "ok" {
                    return Err(invalid("recovery snapshot failed integrity verification"));
                }
            }
            files.push(proof(&safety, format!("history/{name}"))?);
        }
    }
    fsutil::fsync_dir(&history)?;
    let plan = Plan {
        originals: scan(&knowledge, &o.namespace)?,
        origin: o,
        files,
    };
    verify_originals(&plan)?;
    write_record(&safety.join(MANIFEST), &plan)?;
    verify_safety(&safety, &plan, None)?;
    write_record(&operation.join(PLAN), &plan)?;
    Ok(Some(Prepared {
        operation,
        knowledge,
        retained,
        plan,
    }))
}

fn commit_key(id: impl std::fmt::Display) -> String {
    format!("crew.knowledgePurge.commit.v1:{id}")
}

impl Prepared {
    fn locations(&self, kind: Kind) -> (PathBuf, PathBuf) {
        let name = kind.name(&self.plan.origin.namespace);
        (
            self.knowledge.join(&name),
            self.operation.join(LIVE).join(name),
        )
    }
    fn exact(p: &Path, a: &Original) -> Result<bool> {
        metadata(p, a.kind.directory())?
            .map(|m| {
                if m.dev() != a.dev || m.ino() != a.ino {
                    Err(invalid("original artifact replaced"))
                } else {
                    Ok(true)
                }
            })
            .transpose()
            .map(|s| s.unwrap_or(false))
    }
    fn check_directories(&self) -> Result<()> {
        if metadata(&self.knowledge, true)?.is_none()
            || metadata(&self.operation, true)?.is_none()
            || metadata(
                self.retained
                    .parent()
                    .ok_or_else(|| invalid("backup parent missing"))?,
                true,
            )?
            .is_none()
            || metadata(&self.retained, true)?.is_none()
        {
            return Err(invalid("original purge directories differ"));
        }
        metadata(&self.operation.join(LIVE), true)?;
        Ok(())
    }

    pub fn quarantine(&self) -> Result<()> {
        self.check_directories()?;
        for item in &self.plan.originals {
            let (source, staged) = self.locations(item.kind);
            if !Self::exact(&source, item)? || metadata(&staged, item.kind.directory())?.is_some() {
                return Err(invalid("original quarantine cut differs"));
            }
            fsutil::rename_durably(&source, &staged)?;
        }
        self.check_no_live()
    }
    fn check_no_live(&self) -> Result<()> {
        for kind in Kind::ALL {
            if metadata(&self.locations(kind).0, kind.directory())?.is_some() {
                return Err(invalid("a live namespace artifact remains"));
            }
        }
        Ok(())
    }
    pub fn record_commit(&self, c: &Connection, at_ms: i64) -> Result<()> {
        if c.is_autocommit() || crew_exists(c, &self.plan.origin.crew)? || at_ms <= 0 {
            return Err(invalid("purge marker must join exact Core crew deletion"));
        }
        self.check_no_live()?;
        let key = commit_key(&self.plan.origin.operation);
        if meta_get(c, &key)?.is_some() {
            return Err(invalid("original purge marker already exists"));
        }
        let result = Committed {
            plan_sha256: team_data::digest(&self.plan)?,
            committed_at_ms: at_ms,
        };
        meta_set(c, &key, &serde_json::to_string(&result)?)
    }
    /// Reconcile with actual Core outcome; an error before commit never licenses
    /// discard of original files, even if some renames have already happened.
    pub fn settle(&self, c: &Connection) -> Result<bool> {
        self.check_directories()?;
        let marker = meta_get(c, &commit_key(&self.plan.origin.operation))?;
        let safety = self.operation.join(SAFETY);
        let retained = self.retained.join(&self.plan.origin.operation);
        if let Some(marker) = marker {
            let marker: Committed = serde_json::from_str(&marker)?;
            if marker.plan_sha256 != team_data::digest(&self.plan)?
                || marker.committed_at_ms <= 0
                || crew_exists(c, &self.plan.origin.crew)?
            {
                return Err(invalid("Core commit does not match original purge"));
            }
            self.check_no_live()?;
            if metadata(&retained, true)?.is_some() {
                if metadata(&safety, true)?.is_some() {
                    return Err(invalid("duplicate safety pair"));
                }
                verify_safety(&retained, &self.plan, Some(&marker))?;
                if metadata(&retained.join(COMPLETED), false)?.is_none() {
                    return Err(invalid("retained pair lacks a completion proof"));
                }
            } else {
                clean_record_tmp(&safety, COMPLETED)?;
                verify_safety(&safety, &self.plan, Some(&marker))?;
                self.discard_quarantine()?;
                write_record(&safety.join(COMPLETED), &marker)?;
                fsutil::rename_durably(&safety, &retained)?;
            }
            self.discard_quarantine()?;
            finish_pending(&self.operation)?;
            Ok(true)
        } else {
            if !crew_exists(c, &self.plan.origin.crew)? || metadata(&retained, true)?.is_some() {
                return Err(invalid("unknown Core outcome; original artifacts retained"));
            }
            for item in self.plan.originals.iter().rev() {
                let (source, staged) = self.locations(item.kind);
                match (Self::exact(&source, item)?, Self::exact(&staged, item)?) {
                    (true, false) => {}
                    (false, true) => fsutil::rename_durably(&staged, &source)?,
                    _ => return Err(invalid("ambiguous rollback artifact")),
                }
            }
            discard_uncommitted(&self.operation)?;
            Ok(false)
        }
    }
    fn discard_quarantine(&self) -> Result<()> {
        let live = self.operation.join(LIVE);
        if metadata(&live, true)?.is_none() {
            return Ok(());
        }
        let allowed: Vec<_> = self
            .plan
            .originals
            .iter()
            .map(|p| p.kind.name(&self.plan.origin.namespace))
            .collect();
        if names(&live, Kind::ALL.len())?
            .iter()
            .any(|n| !allowed.contains(n))
        {
            return Err(invalid("unexpected quarantined artifact"));
        }
        for item in &self.plan.originals {
            let path = self.locations(item.kind).1;
            if Self::exact(&path, item)? {
                if item.kind.directory() {
                    for name in names(&path, MAX_HISTORY_FILES)? {
                        history_name(&name)?;
                        let found = proof(&path, name.clone())?;
                        if !self.plan.files.iter().any(|p| {
                            p.name.strip_prefix("history/") == Some(name.as_str())
                                && p.bytes == found.bytes
                                && p.sha256 == found.sha256
                        }) {
                            return Err(invalid(
                                "quarantined history differs from the safety copy",
                            ));
                        }
                        fs::remove_file(path.join(name))?;
                    }
                    fsutil::fsync_dir(&path)?;
                    fs::remove_dir(path)?;
                } else {
                    fs::remove_file(path)?;
                }
            }
        }
        fsutil::fsync_dir(&live)?;
        fs::remove_dir(&live)?;
        fsutil::fsync_dir(&self.operation)?;
        Ok(())
    }
}

fn clean_record_tmp(root: &Path, basename: &str) -> Result<()> {
    let prefix = format!(".{basename}.tmp-");
    for name in names(root, 8)? {
        if let Some(pid) = name.strip_prefix(&prefix) {
            if !pid
                .parse::<u32>()
                .is_ok_and(|n| n > 0 && n.to_string() == pid)
            {
                return Err(invalid("unknown manifest staging file"));
            }
            let path = root.join(name);
            metadata(&path, false)?.ok_or_else(|| invalid("manifest staging file missing"))?;
            fs::remove_file(path)?;
            fsutil::fsync_dir(root)?;
        }
    }
    Ok(())
}

fn finish_pending(op: &Path) -> Result<()> {
    // Rename away the operation as one durable step. The remaining contents
    // are only app-created copies/metadata and no longer drive recovery.
    let parent = op
        .parent()
        .ok_or_else(|| invalid("pending operation parent missing"))?;
    let name = op
        .file_name()
        .ok_or_else(|| invalid("pending operation identity missing"))?
        .to_string_lossy();
    let done = parent.join(format!("done-{name}"));
    if metadata(&done, true)?.is_some() {
        return Err(invalid("operation cleanup already exists"));
    }
    fsutil::rename_durably(op, &done)?;
    clean_done(&done)
}

fn known_temp(name: &str, files: &[&str]) -> bool {
    files.iter().any(|file| {
        name.strip_prefix(&format!(".{file}.tmp-"))
            .is_some_and(|pid| {
                pid.parse::<u32>()
                    .is_ok_and(|n| n > 0 && n.to_string() == pid)
            })
    })
}

fn verify_disposable(root: &Path, pending: bool) -> Result<()> {
    metadata(root, true)?.ok_or_else(|| invalid("cleanup directory missing"))?;
    let files = if pending {
        &[MANIFEST, PLAN][..]
    } else {
        &[CORE, KNOWLEDGE, MANIFEST, COMPLETED][..]
    };
    for name in names(root, 16)? {
        let path = root.join(&name);
        if pending && name == SAFETY {
            verify_disposable(&path, false)?;
        } else if pending && name == LIVE {
            if metadata(&path, true)?.is_none() || !names(&path, 1)?.is_empty() {
                return Err(invalid("cleanup still contains live artifacts"));
            }
        } else if !pending && name == HISTORY {
            metadata(&path, true)?.ok_or_else(|| invalid("cleanup history missing"))?;
            for child in names(&path, MAX_HISTORY_FILES)? {
                history_name(&child)?;
                metadata(&path.join(child), false)?
                    .ok_or_else(|| invalid("cleanup snapshot missing"))?;
            }
        } else if files.contains(&name.as_str()) || known_temp(&name, files) {
            metadata(&path, false)?.ok_or_else(|| invalid("cleanup file missing"))?;
        } else {
            return Err(invalid("unexpected cleanup artifact"));
        }
    }
    Ok(())
}

fn clean_done(done: &Path) -> Result<()> {
    let parent = done
        .parent()
        .ok_or_else(|| invalid("cleanup parent missing"))?;
    verify_disposable(
        done,
        parent.file_name().and_then(|n| n.to_str()) == Some(PENDING),
    )?;
    fs::remove_dir_all(done)?;
    fsutil::fsync_dir(parent)?;
    Ok(())
}

fn discard_uncommitted(op: &Path) -> Result<()> {
    let live = op.join(LIVE);
    if metadata(&live, true)?.is_some() && !names(&live, Kind::ALL.len())?.is_empty() {
        return Err(invalid("rollback left original files in quarantine"));
    }
    finish_pending(op)
}

pub(crate) fn has_recovery_material(data: &Path) -> Result<bool> {
    let pending = data.join(PENDING);
    let retained = data.join(backup::BACKUPS_DIR).join(RETAINED);
    for dir in [pending, retained] {
        if metadata(&dir, true)?.is_some() && fs::read_dir(dir)?.next().transpose()?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Must run before admitting any request and before the backup worker starts.
pub fn recover(store: &Store, data: &Path) -> Result<()> {
    let pending = data.join(PENDING);
    if metadata(&pending, true)?.is_none() {
        return Ok(());
    }
    let database =
        meta_get(store.conn(), "dbId")?.ok_or_else(|| invalid("Core identity missing"))?;
    if store.backups_dir().is_none() {
        return Err(invalid("backup directory required"));
    }
    let restore_stamp = crate::personal_drafts::epoch(store.conn())?;
    let retained = data.join(backup::BACKUPS_DIR).join(RETAINED);
    for name in names(&pending, MAX_PENDING)? {
        let path = pending.join(&name);
        if let Some(id) = name.strip_prefix("done-") {
            uuid_name(id)?;
            clean_done(&path)?;
            continue;
        }
        let id = uuid_name(&name)?;
        metadata(&path, true)?.ok_or_else(|| invalid("pending directory missing"))?;
        let origin_path = path.join(MANIFEST);
        if metadata(&origin_path, false)?.is_none() {
            clean_record_tmp(&path, MANIFEST)?;
            if names(&path, 1)?.is_empty() {
                finish_pending(&path)?;
                continue;
            }
            return Err(invalid("pending operation lost original manifest"));
        }
        let origin: Origin = bounded_read(&origin_path)?;
        valid_origin(&origin, &database, &restore_stamp, id)?;
        let plan_path = path.join(PLAN);
        if metadata(&plan_path, false)?.is_none() {
            if meta_get(store.conn(), &commit_key(id))?.is_some()
                || !crew_exists(store.conn(), &origin.crew)?
            {
                return Err(invalid("unprepared purge has unexpected Core outcome"));
            }
            discard_uncommitted(&path)?;
            continue;
        }
        let plan: Plan = bounded_read(&plan_path)?;
        if plan.origin != origin {
            return Err(invalid("original plan mismatch"));
        }
        verify_originals(&plan)?;
        Prepared {
            operation: path,
            knowledge: data.join("knowledge"),
            retained: retained.clone(),
            plan,
        }
        .settle(store.conn())?;
    }
    Ok(())
}

pub(crate) fn expire_completed(
    backups: &Path,
    now_ms: i64,
    policy: backup::Policy,
) -> Result<Vec<PathBuf>> {
    let retained = backups.join(RETAINED);
    if metadata(&retained, true)?.is_none() {
        return Ok(Vec::new());
    }
    let pending = backups
        .parent()
        .ok_or_else(|| invalid("retention data parent missing"))?
        .join(PENDING);
    let pending_exists = metadata(&pending, true)?.is_some();
    let mut removed = Vec::new();
    for item in fs::read_dir(&retained)? {
        let item = item?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| invalid("unexpected safety pair name"))?;
        let path = item.path();
        if let Some(id) = name.strip_prefix("done-") {
            uuid_name(id)?;
            clean_done(&path)?;
            continue;
        }
        let id = uuid_name(&name)?;
        metadata(&path, true)?.ok_or_else(|| invalid("retained pair disappeared"))?;
        // Publication precedes the final pending rename. A failed finalize may
        // still need this exact pair for restart, even after its retention age.
        if pending_exists && metadata(&pending.join(&name), true)?.is_some() {
            continue;
        }
        let plan: Plan = bounded_read(&path.join(MANIFEST))?;
        valid_origin(
            &plan.origin,
            &plan.origin.database,
            &plan.origin.restore_stamp,
            id,
        )?;
        let committed: Committed = bounded_read(&path.join(COMPLETED))?;
        if committed.plan_sha256 != team_data::digest(&plan)? || committed.committed_at_ms <= 0 {
            return Err(invalid("retention requires a verified completion proof"));
        }
        if now_ms.saturating_sub(committed.committed_at_ms) > policy.keep_pre_delete_ms {
            verify_safety(&path, &plan, Some(&committed))?;
            // An interrupted expiration may leave the private expiry name.
            let expired = retained.join(format!("done-{name}"));
            if metadata(&expired, true)?.is_some() {
                return Err(invalid("retention cleanup already exists"));
            }
            fsutil::rename_durably(&path, &expired)?;
            clean_done(&expired)?;
            removed.push(path);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store, DataContext) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().canonicalize().unwrap();
        let store = Store::open_data_dir(&data).unwrap();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at,archived_at) VALUES('source','retained prior row','t','t','t'),('neighbor','unchanged','t','t',NULL)").unwrap();
        let database = meta_get(store.conn(), "dbId").unwrap().unwrap();
        let ns = team_data::namespace(&database, "source");
        let ctx = DataContext {
            db_id: database.clone(),
            authority_id: database,
            restore_epoch: "initial".into(),
            crew_id: "source".into(),
            namespace: ns,
            generation: "0".into(),
        };
        let knowledge = data.join("knowledge");
        directory(&knowledge).unwrap();
        let path = knowledge.join(Kind::Database.name(&ctx.namespace));
        let workspace = team_data::Knowledge::open(&path, ctx.clone(), true).unwrap();
        workspace
            .conn
            .execute_batch(
                "CREATE TABLE actual_rows(id INTEGER PRIMARY KEY, label TEXT NOT NULL);
            INSERT INTO actual_rows VALUES(1,'retained before deletion');",
            )
            .unwrap();
        workspace.snapshot().unwrap();
        drop(workspace);
        (root, store, ctx)
    }

    fn data(root: &tempfile::TempDir) -> PathBuf {
        root.path().canonicalize().unwrap()
    }

    #[test]
    fn prepared_reversible_move_and_partial_quarantine_restore_exact_originals_after_reopen() {
        let (root, store, ctx) = fixture();
        let data = data(&root);
        let database = data
            .join("knowledge")
            .join(Kind::Database.name(&ctx.namespace));
        let initial = fs::read(&database).unwrap();
        let prepared = prepare(&store, &data, "source").unwrap().unwrap();
        let (source, quarantine) = prepared.locations(Kind::Database);
        fsutil::rename_durably(&source, &quarantine).unwrap();
        assert!(!source.exists() && quarantine.exists());
        assert!(crew_exists(store.conn(), "source").unwrap());
        drop(store);

        let reopened = Store::open_data_dir(&data).unwrap();
        recover(&reopened, &data).unwrap();
        assert_eq!(fs::read(&database).unwrap(), initial);
        assert!(crew_exists(reopened.conn(), "source").unwrap());
        assert!(names(&data.join(PENDING), MAX_PENDING).unwrap().is_empty());
        assert!(
            names(&data.join(backup::BACKUPS_DIR).join(RETAINED), 2)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn committed_crash_finishes_exact_cleanup_and_retains_recoverable_pair_until_expiry() {
        let (root, mut store, ctx) = fixture();
        let data = data(&root);
        let live_root = data.join("knowledge");
        let neighbor_ns = team_data::namespace(&ctx.db_id, "neighbor");
        let neighbor = live_root.join(Kind::Database.name(&neighbor_ns));
        let mut neighbor_ctx = ctx.clone();
        neighbor_ctx.crew_id = "neighbor".into();
        neighbor_ctx.namespace = neighbor_ns;
        let other = team_data::Knowledge::open(&neighbor, neighbor_ctx, true).unwrap();
        other.conn.execute_batch("CREATE TABLE neighbor_only(id INTEGER PRIMARY KEY); INSERT INTO neighbor_only VALUES(37)").unwrap();
        drop(other);
        let neighbor_before = fs::read(&neighbor).unwrap();

        let history_root = live_root.join(Kind::History.name(&ctx.namespace));
        let snapshot_name = names(&history_root, MAX_HISTORY_FILES).unwrap().remove(0);
        let snapshot = history_root.join(&snapshot_name);
        // Use the exact read strategy of older Knowledge::restore.
        let legacy = read_db(&snapshot).unwrap();
        assert_eq!(
            legacy
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        drop(legacy);
        assert!(team_data::legacy_snapshot_sidecars(&snapshot).unwrap());
        let legacy_index = fs::read(history_root.join(format!("{snapshot_name}-shm"))).unwrap();

        let prepared = prepare(&store, &data, "source").unwrap().unwrap();
        let original = prepared.plan.origin.operation.clone();
        let at = crate::now_ms();
        prepared.quarantine().unwrap();
        store
            .with_tx(|tx| {
                tx.execute("DELETE FROM crews WHERE id='source'", [])?;
                prepared.record_commit(tx, at)
            })
            .unwrap();
        let (_, path) = prepared.locations(Kind::Database);
        assert!(path.exists());
        assert!(!crew_exists(store.conn(), "source").unwrap());
        // Model an interruption after unlinking the main quarantined file.
        fs::remove_file(&path).unwrap();
        fsutil::fsync_dir(path.parent().unwrap()).unwrap();
        drop(store);

        let reopened = Store::open_data_dir(&data).unwrap();
        recover(&reopened, &data).unwrap();
        recover(&reopened, &data).unwrap();
        assert!(names(&data.join(PENDING), MAX_PENDING).unwrap().is_empty());
        assert_eq!(fs::read(&neighbor).unwrap(), neighbor_before);
        for kind in Kind::ALL {
            assert!(!live_root.join(kind.name(&ctx.namespace)).exists());
        }
        let backup_dir = data.join(backup::BACKUPS_DIR);
        let saved = backup_dir.join(RETAINED).join(&original);
        let core = read_snapshot(&saved.join(CORE)).unwrap();
        assert!(crew_exists(&core, "source").unwrap());
        let knowledge = read_snapshot(&saved.join(KNOWLEDGE)).unwrap();
        assert_eq!(
            knowledge
                .query_row("SELECT label FROM actual_rows WHERE id=1", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "retained before deletion"
        );
        let history = names(&saved.join(HISTORY), MAX_HISTORY_FILES).unwrap();
        assert_eq!(
            history,
            [
                snapshot_name.clone(),
                format!("{snapshot_name}-shm"),
                format!("{snapshot_name}-wal"),
            ]
        );
        let saved_history = saved.join(HISTORY);
        assert_eq!(
            fs::read(saved_history.join(format!("{snapshot_name}-shm"))).unwrap(),
            legacy_index
        );
        let old = team_data::open_recovery_snapshot(&saved_history.join(&snapshot_name)).unwrap();
        assert_eq!(
            old.query_row("SELECT count(*) FROM actual_rows", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop((old, knowledge, core));
        let policy = backup::Policy::default();
        assert!(
            expire_completed(&backup_dir, at + policy.keep_pre_delete_ms, policy)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            expire_completed(&backup_dir, at + policy.keep_pre_delete_ms + 1, policy).unwrap(),
            [saved]
        );
        assert_eq!(fs::read(&neighbor).unwrap(), neighbor_before);
    }

    #[test]
    fn expired_pair_is_pinned_by_pending_finalization_until_recovery_finishes() {
        let (root, mut store, _) = fixture();
        let data = data(&root);
        let prepared = prepare(&store, &data, "source").unwrap().unwrap();
        let at = crate::now_ms();
        prepared.quarantine().unwrap();
        store
            .with_tx(|tx| {
                tx.execute("DELETE FROM crews WHERE id='source'", [])?;
                prepared.record_commit(tx, at)
            })
            .unwrap();

        // The exact bundle was published, but cleanup of the pending operation
        // failed before its final rename. The backup ticker may run independently.
        let marker: Committed = serde_json::from_str(
            &meta_get(store.conn(), &commit_key(&prepared.plan.origin.operation))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        prepared.discard_quarantine().unwrap();
        let safety = prepared.operation.join(SAFETY);
        write_record(&safety.join(COMPLETED), &marker).unwrap();
        let retained = prepared.retained.join(&prepared.plan.origin.operation);
        fsutil::rename_durably(&safety, &retained).unwrap();
        assert!(prepared.operation.is_dir());
        let backups = data.join(backup::BACKUPS_DIR);
        let policy = backup::Policy::default();
        let expired = at + policy.keep_pre_delete_ms + 1;
        assert!(
            expire_completed(&backups, expired, policy)
                .unwrap()
                .is_empty()
        );
        assert!(retained.is_dir());

        drop(store);
        let reopened = Store::open_data_dir(&data).unwrap();
        recover(&reopened, &data).unwrap();
        assert!(!prepared.operation.exists());
        assert!(retained.is_dir());
        assert_eq!(
            expire_completed(&backups, expired, policy).unwrap(),
            [retained]
        );
    }

    #[test]
    fn backup_or_identity_failure_never_mutates_live_namespace_and_unknown_artifacts_fail_closed() {
        let (root, store, ctx) = fixture();
        let data = data(&root);
        let live = data.join("knowledge");
        let database = live.join(Kind::Database.name(&ctx.namespace));
        let bytes = fs::read(&database).unwrap();
        let history = live.join(Kind::History.name(&ctx.namespace));
        let unknown = history.join("foreign.sqlite");
        fs::write(&unknown, b"foreign").unwrap();
        assert!(prepare(&store, &data, "source").is_err());
        recover(&store, &data).unwrap();
        assert!(crew_exists(store.conn(), "source").unwrap());
        assert_eq!(fs::read(&database).unwrap(), bytes);
        assert_eq!(fs::read(&unknown).unwrap(), b"foreign");
        assert!(names(&data.join(PENDING), MAX_PENDING).unwrap().is_empty());

        let foreign = live.join(Kind::Wal.name(&ctx.namespace));
        std::os::unix::fs::symlink(&database, &foreign).unwrap();
        assert!(prepare(&store, &data, "source").is_err());
        assert_eq!(fs::read(&database).unwrap(), bytes);
        assert!(fs::symlink_metadata(foreign).unwrap().is_symlink());
    }
}
