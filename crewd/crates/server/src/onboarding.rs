//! Closed startup publication and derived offline guide projection.
//! No model/session control. Knowledge is the sole editable content authority.
use crewd_collab::{self as collab, onboarding as facts};
use crewd_proto::{ResourceActor, team_data::*};
use crewd_store::{
    Store,
    team_data::{Capture, Knowledge, Mutation},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

const EDITION: u32 = 1;
const EXPORT_KEY: &str = "onboarding.export.v1";
const BUNDLED: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("onboarding/assets/SKILL.md")),
    ("INDEX.md", include_str!("onboarding/assets/INDEX.md")),
    ("INDEX-v2.md", include_str!("onboarding/assets/INDEX-v2.md")),
    (
        "README-source-v1.md",
        include_str!("onboarding/assets/README-source-v1.md"),
    ),
    (
        "README-public-v2.md",
        include_str!("onboarding/assets/README-public-v2.md"),
    ),
    (
        "README_EN-public-v1.md",
        include_str!("onboarding/assets/README_EN-public-v1.md"),
    ),
    (
        "architecture-source-v1.md",
        include_str!("onboarding/assets/architecture-source-v1.md"),
    ),
    ("guide-v1.md", include_str!("onboarding/assets/guide-v1.md")),
    (
        "CORE-guide-v1.md",
        include_str!("onboarding/assets/CORE-guide-v1.md"),
    ),
];
fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn assets() -> Vec<facts::Asset> {
    BUNDLED
        .iter()
        .map(|(file, body)| facts::Asset {
            file: (*file).into(),
            sha256: sha(body.as_bytes()),
            bytes: body.len() as u64,
        })
        .collect()
}
fn base(db: &str) -> DataContext {
    DataContext {
        db_id: db.into(),
        authority_id: db.into(),
        restore_epoch: "initial".into(),
        crew_id: facts::ROOT.into(),
        namespace: crewd_store::team_data::namespace(db, facts::ROOT),
        generation: "0".into(),
    }
}
fn body(page: &facts::Page) -> anyhow::Result<&'static str> {
    let text = BUNDLED
        .iter()
        .find(|(name, _)| *name == page.asset.file)
        .map(|(_, text)| *text)
        .ok_or_else(|| anyhow::anyhow!("original onboarding version no longer bundled"))?;
    anyhow::ensure!(
        sha(text.as_bytes()) == page.asset.sha256 && text.len() as u64 == page.asset.bytes,
        "original onboarding bytes changed; retain pending plan"
    );
    Ok(text)
}
fn mutations(p: &facts::Publication) -> anyhow::Result<Vec<Mutation>> {
    let mut changes = Vec::new();
    for page in &p.pages {
        let provenance = DataProvenance {
            created_by: ResourceActor {
                kind: "human".into(),
                session_id: None,
            },
            updated_by: ResourceActor {
                kind: "human".into(),
                session_id: None,
            },
            created_at: p.created_at.clone(),
            updated_at: p.created_at.clone(),
            sources: vec![],
            attributes: json!({"origin":"bundledPublicOnboarding","sourceEdition":p.edition,"sourceFile":page.asset.file,"sourceSHA256":page.asset.sha256,"historicalSourceCommit":if page.asset.file == "CORE-guide-v1.md" { "5daf5a26" } else { "e427fb2" }}),
        };
        changes.push(Mutation::PutObject {
            object: Box::new(DataObject::Block {
                id: page.block.clone(),
                revision: "1".into(),
                content_type: "text/plain".into(),
                content: Value::String(body(page)?.into()),
                archived: false,
                provenance: provenance.clone(),
            }),
            expected_revision: None,
            captures: vec![],
        });
        changes.push(Mutation::PutObject {
            object: Box::new(DataObject::View {
                id: page.view.clone(),
                revision: "1".into(),
                title: page.asset.file.clone(),
                definition: DataViewDefinition::Document {
                    placements: vec![DataPlacement {
                        id: page.placement.clone(),
                        block: DataRefState::Visible {
                            reference: DataRef {
                                namespace: p.origin.namespace.clone(),
                                kind: "block".into(),
                                id: page.block.clone(),
                            },
                        },
                        at_revision: None,
                        layout: json!({}),
                    }],
                    layout: json!({}),
                },
                archived: false,
                provenance,
            }),
            expected_revision: None,
            captures: vec![Capture {
                anchor: format!("placement:{}", page.placement),
                namespace: p.origin.namespace.clone(),
                kind: "block".into(),
                id: page.block.clone(),
                commit: "self".into(),
                revision: "1".into(),
                live: true,
            }],
        });
    }
    Ok(changes)
}
/// Caller owns the original data-directory lock; there is no admitted service
/// or independent executor at this boundary. Missing files on a resumed plan
/// are NOT evidence that the original operation never committed.
pub(crate) fn bootstrap(store: &mut Store, data: &Path) -> anyhow::Result<()> {
    // Revoke trusted publication BEFORE canonicalization, Knowledge open or any
    // other fallible external operation, including the existing-ready path.
    invalidate_existing(store.conn())?;
    bootstrap_inner(store, data)
}
fn bootstrap_inner(store: &mut Store, data: &Path) -> anyhow::Result<()> {
    let data = fs::canonicalize(data)?;
    let (mut state, _) = collab::run(store, crewd_store::now_ms(), facts::begin)?;
    if state.skipped {
        return Ok(());
    }
    let root = data.join("knowledge");
    crewd_store::team_data::private_dir(&root)?;
    let knowledge_path = root.join(format!(
        "{}.sqlite",
        crewd_store::team_data::namespace(&state.db_id, facts::ROOT)
    ));
    let resumed = state.pending.is_some();
    if state.ready.is_empty() && !resumed {
        anyhow::ensure!(
            !knowledge_path.try_exists()?,
            "pre-existing onboarding namespace is not owned by the original plan"
        );
    }
    // An intentionally removed tutorial is never materialized again.
    if !state.ready.is_empty()
        && facts::root_lifecycle(store.conn())? != facts::RootLifecycle::Active
    {
        if let Err(error) = project_committed(store.conn(), &data) {
            if error
                .downcast_ref::<StatusQualificationFailure>()
                .is_some_and(|failure| failure.revocation_unknown)
            {
                return Err(error);
            }
            eprintln!(
                "crewd: onboarding projection inactive/cleanup unresolved; Core lifecycle preserved: {error}"
            );
        }
        return Ok(());
    }
    let origin = if let Some(p) = &state.pending {
        p.origin.clone()
    } else if state.ready.is_empty() {
        base(&state.db_id)
    } else {
        let k = Knowledge::open(&knowledge_path, base(&state.db_id), false)?;
        k.context()?
    };
    let (next, _) = collab::run(store, crewd_store::now_ms(), |tx| {
        facts::prepare(tx, EDITION, &assets(), origin)
    })?;
    state = next;
    if let Some(p) = state.pending.clone() {
        if resumed {
            anyhow::ensure!(
                knowledge_path.try_exists()?,
                "original onboarding knowledge disappeared; unresolved original request retained without replay"
            );
        }
        let mut k = Knowledge::open(&knowledge_path, base(&state.db_id), !resumed)?;
        let receipt = if let Some(r) =
            k.retry(&p.request, &p.origin, facts::PRINCIPAL, &p.payload_sha256)?
        {
            r
        } else {
            // A missing receipt alone never permits replay. Require the exact
            // pristine first-generation namespace and no other original facts.
            anyhow::ensure!(
                k.context()? == p.origin,
                "onboarding outcome unknown; original generation/restore scope changed"
            );
            if state.ready.is_empty() {
                anyhow::ensure!(
                    p.origin.generation == "0",
                    "first onboarding generation is not pristine"
                );
                for table in [
                    "_pc_objects",
                    "_pc_revisions",
                    "_pc_receipts",
                    "_pc_previews",
                    "_pc_snapshots",
                ] {
                    let count: i64 =
                        k.conn
                            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                                r.get(0)
                            })?;
                    anyhow::ensure!(
                        count == 0,
                        "onboarding namespace contains non-plan knowledge; original retained"
                    );
                }
            } else {
                // A completed own namespace can gain only new version objects.
                // Equal original generation/restore epoch proves this batch has
                // not committed; fixed planned IDs must additionally be absent.
                for page in &p.pages {
                    for (kind, id) in [("block", &page.block), ("view", &page.view)] {
                        let count: i64 = k.conn.query_row("SELECT (SELECT count(*) FROM _pc_objects WHERE kind=?1 AND id=?2)+(SELECT count(*) FROM _pc_revisions WHERE kind=?1 AND id=?2)", rusqlite::params![kind,id], |r| r.get(0))?;
                        anyhow::ensure!(
                            count == 0,
                            "onboarding planned identity already contains non-plan knowledge"
                        );
                    }
                }
            }
            k.apply(
                &p.request,
                &p.origin,
                facts::PRINCIPAL,
                &p.payload_sha256,
                BTreeMap::new(),
                mutations(&p)?,
                false,
                None,
            )?
        };
        anyhow::ensure!(
            k.context()? == receipt.context,
            "onboarding knowledge changed after original publication; retained without adoption"
        );
        if state.ready.is_empty() {
            for table in ["_pc_objects", "_pc_revisions"] {
                let count: i64 =
                    k.conn
                        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
                anyhow::ensure!(
                    count == (2 * p.pages.len()) as i64,
                    "onboarding namespace contains non-plan objects"
                );
            }
            let receipts: i64 = k
                .conn
                .query_row("SELECT count(*) FROM _pc_receipts", [], |r| r.get(0))?;
            anyhow::ensure!(
                receipts == 1,
                "onboarding namespace contains non-plan receipts"
            );
        }
        // Compare the complete original resource and capture bytes, not merely
        // a familiar title/body hash or a retained receipt next to alien rows.
        for mutation in mutations(&p)? {
            let Mutation::PutObject {
                object, captures, ..
            } = mutation
            else {
                unreachable!()
            };
            let (kind, id, _, _) = crewd_store::team_data::object_fields(&object);
            let reference = DataRef {
                namespace: p.origin.namespace.clone(),
                kind: kind.into(),
                id: id.into(),
            };
            let original = k.get(&reference, Some("1"))?;
            anyhow::ensure!(
                serde_json::to_value(&original.object)? == serde_json::to_value(object.as_ref())?
                    && serde_json::to_value(&original.captures)?
                        == serde_json::to_value(&captures)?,
                "onboarding complete original resource/capture witness differs"
            );
        }
        // Knowledge::apply has committed and released its writer transaction.
        anyhow::ensure!(
            k.conn.is_autocommit(),
            "knowledge writer transaction retained at publication boundary"
        );
        collab::run(store, crewd_store::now_ms(), |tx| {
            facts::finish(tx, &receipt)
        })?;
    }
    // Projection is a separate derived operation; its failure never changes a
    // committed Core result into a refused business mutation.
    if let Err(error) = project_committed(store.conn(), &data) {
        if error
            .downcast_ref::<StatusQualificationFailure>()
            .is_some_and(|failure| failure.revocation_unknown)
        {
            return Err(error);
        }
        eprintln!(
            "crewd: onboarding offline projection not ready; retained for startup/commit reconciliation: {error}"
        );
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportFile {
    path: String,
    sha256: String,
    bytes: u64,
    device: Option<u64>,
    inode: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Projection {
    format: u32,
    db_id: String,
    state: String,
    generation: Option<String>,
    manifests: Vec<ExportFile>,
    owned: Vec<ExportFile>,
    current: Option<ExportFile>,
    status: Option<ExportFile>,
    #[serde(default)]
    retired_current: Option<ExportFile>,
    #[serde(default)]
    reserved_current: Option<ExportFile>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportPage {
    file: String,
    export_path: String,
    view_id: String,
    view_revision: String,
    title: String,
    blocks: Vec<(String, String)>,
    bytes: u64,
    sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format: u32,
    db_id: String,
    crew_id: String,
    namespace: String,
    restore_epoch: String,
    generation: String,
    historical_bundle_source: String,
    pages: Vec<ExportPage>,
}
struct BodyProjection {
    file: String,
    path: String,
    digest: String,
    bytes: u64,
    blocks: Vec<(DataRef, String)>,
}
fn write_body(
    k: &Knowledge,
    blocks: &[(DataRef, String)],
    file: &mut fs::File,
) -> anyhow::Result<()> {
    for (reference, at) in blocks {
        let stored = k.get(reference, Some(at))?;
        let DataObject::Block {
            id,
            revision,
            content_type,
            content: Value::String(text),
            archived: false,
            ..
        } = stored.object
        else {
            anyhow::bail!("original export block revision unavailable")
        };
        anyhow::ensure!(
            id == reference.id && revision == *at && content_type == "text/plain",
            "original export block revision changed"
        );
        file.write_all(text.as_bytes())?;
    }
    Ok(())
}
const FILE_LIMIT: usize = 4096;
const EXPORT_BYTES_LIMIT: usize = 8 * 1024 * 1024;
fn validate_projection(c: &rusqlite::Connection, p: &Projection) -> anyhow::Result<String> {
    anyhow::ensure!(
        p.format == 1
            && crewd_store::meta_get(c, "dbId")?.as_deref() == Some(p.db_id.as_str())
            && p.db_id.len() <= 4096
            && p.generation
                .as_ref()
                .is_none_or(|v| v.parse::<u64>().is_ok())
            && p.owned.len() <= FILE_LIMIT
            && p.manifests.len() <= FILE_LIMIT,
        "onboarding projection scope or bound invalid; original retained"
    );
    anyhow::ensure!(
        [
            "pending",
            "ready",
            "inactive",
            "cleanupPending",
            "cleanupUnverified",
            "cleared"
        ]
        .contains(&p.state.as_str()),
        "onboarding projection state invalid"
    );
    for f in p
        .owned
        .iter()
        .chain(&p.manifests)
        .chain(p.current.iter())
        .chain(p.status.iter())
        .chain(p.retired_current.iter())
        .chain(p.reserved_current.iter())
    {
        anyhow::ensure!(
            clean_name(&f.path)
                && f.path.len() <= 512
                && f.sha256.len() == 64
                && f.sha256
                    .bytes()
                    .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
                && f.device.is_some() == f.inode.is_some(),
            "onboarding export receipt invalid"
        );
    }
    let raw = serde_json::to_string(p)?;
    anyhow::ensure!(
        raw.len() <= EXPORT_BYTES_LIMIT,
        "onboarding projection byte bound exceeded"
    );
    Ok(raw)
}
fn save(c: &rusqlite::Connection, p: &Projection) -> anyhow::Result<()> {
    anyhow::ensure!(
        p.current.is_none()
            || (p.state == "ready"
                && p.generation.is_some()
                && p.retired_current.is_none()
                && p.reserved_current.is_none()),
        "unqualified current publication"
    );
    let raw = validate_projection(c, p)?;
    let tx = c.unchecked_transaction()?;
    crewd_store::meta_set(&tx, EXPORT_KEY, &raw)?;
    tx.commit()?;
    Ok(())
}
fn load(c: &rusqlite::Connection) -> anyhow::Result<Option<Projection>> {
    let Some(raw) = crewd_store::meta_get(c, EXPORT_KEY)? else {
        return Ok(None);
    };
    anyhow::ensure!(
        raw.len() <= EXPORT_BYTES_LIMIT,
        "onboarding projection byte bound exceeded"
    );
    let p: Projection = serde_json::from_str(&raw)?;
    validate_projection(c, &p)?;
    Ok(Some(p))
}
fn invalidate_existing(c: &rusqlite::Connection) -> anyhow::Result<()> {
    let Some(mut p) = load(c)? else { return Ok(()) };
    if let Some(old) = p.current.take() {
        // An old interrupted (non-ready) Current is only a reservation. Never
        // promote it through the unchanged-generation reuse path.
        let slot = if p.state == "ready" {
            &mut p.retired_current
        } else {
            &mut p.reserved_current
        };
        anyhow::ensure!(
            slot.is_none(),
            "onboarding publication ownership ambiguous; readiness unknown"
        );
        *slot = Some(old);
    }
    if p.state == "ready" {
        p.state = "pending".into();
    }
    p.generation = None;
    save(c, &p)
}
#[cfg(test)]
type StatusAction = Box<dyn FnOnce(&rusqlite::Connection, &Path)>;
#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
    static STATUS_ACTION: std::cell::RefCell<Option<StatusAction>> = const { std::cell::RefCell::new(None) };
    static UNLINK_FAULT_PATH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}
fn checkpoint(point: &str) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAULT.with(|f| f.borrow().as_ref().is_some_and(|v| *v == point)) {
        anyhow::bail!("controlled onboarding IO failure: {point}");
    }
    let _ = point;
    Ok(())
}
fn clean_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && s.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
fn secure_dir(path: &Path) -> anyhow::Result<()> {
    checkpoint("directory")?;
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
        }
        Err(e) => return Err(e.into()),
        Ok(_) => (),
    }
    let m = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        m.is_dir()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o077 == 0
            && fs::canonicalize(path)? == path,
        "onboarding export directory ownership differs"
    );
    Ok(())
}
fn matches(root: &Path, f: &ExportFile) -> anyhow::Result<bool> {
    anyhow::ensure!(
        clean_name(&f.path),
        "onboarding export receipt path invalid"
    );
    verify_dir(root)?;
    let path = root.join(&f.path);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("export parent missing"))?;
    let mut cursor = root.to_path_buf();
    if let Ok(parts) = parent.strip_prefix(root) {
        for part in parts {
            cursor.push(part);
            let m = match fs::symlink_metadata(&cursor) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(e) => return Err(e.into()),
            };
            if !m.is_dir()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o077 != 0
                || fs::canonicalize(&cursor)? != cursor
            {
                return Ok(false);
            }
        }
    }
    let info = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    // open(O_NOFOLLOW) and compare identities, never follow a substituted leaf.
    if f.device != Some(info.dev())
        || f.inode != Some(info.ino())
        || !info.is_file()
        || info.nlink() != 1
        || info.uid() != unsafe { libc::geteuid() }
        || info.mode() & 0o077 != 0
        || info.len() != f.bytes
    {
        return Ok(false);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let opened = file.metadata()?;
    if opened.dev() != info.dev() || opened.ino() != info.ino() {
        return Ok(false);
    }
    let mut limited = file.take(
        f.bytes
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("export byte count exhausted"))?,
    );
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut total = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        let count = limited.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| anyhow::anyhow!("export byte count exhausted"))?;
    }
    let actual: String = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(total == f.bytes && actual == f.sha256)
}
#[derive(Clone, Copy)]
enum Ownership {
    Body,
    Manifest,
    Current,
    Status,
}
/// Persist the exact create_new inode claim BEFORE any body bytes are written.
/// Pre-existing identical bytes are not adopted; a reservation without this
/// original inode is not sufficient to delete or publish a filesystem object.
fn publish(
    c: &rusqlite::Connection,
    root: &Path,
    path: &str,
    body: &[u8],
    p: &mut Projection,
    kind: Ownership,
) -> anyhow::Result<bool> {
    publish_stream(
        c,
        root,
        path,
        &sha(body),
        body.len() as u64,
        p,
        kind,
        |file| {
            file.write_all(body)?;
            Ok(())
        },
    )
}
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit original ownership claim and complete streaming body at the derived filesystem boundary"
)]
fn publish_stream(
    c: &rusqlite::Connection,
    root: &Path,
    path: &str,
    digest: &str,
    bytes: u64,
    p: &mut Projection,
    kind: Ownership,
    write: impl FnOnce(&mut fs::File) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    anyhow::ensure!(clean_name(path), "onboarding export path invalid");
    let mut prospective = p.clone();
    put_claim(
        &mut prospective,
        kind,
        ExportFile {
            path: path.into(),
            sha256: digest.into(),
            bytes,
            device: Some(u64::MAX),
            inode: Some(u64::MAX),
        },
    )?;
    validate_projection(c, &prospective)?;
    // A path-only reservation survives a crash between create_new and inode
    // capture. It can never qualify ownership or publication by matching bytes.
    put_claim(p, kind, record(path, digest, bytes))?;
    save(c, p)?;
    verify_dir(root)?;
    let target = root.join(path);
    verify_parent(root, &target)?;
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if matches!(kind, Ownership::Current) {
        checkpoint("current.unclaimed")?;
    }
    let info = file.metadata()?;
    let record = ExportFile {
        path: path.into(),
        sha256: digest.into(),
        bytes,
        device: Some(info.dev()),
        inode: Some(info.ino()),
    };
    put_claim(p, kind, record)?;
    save(c, p)?;
    if matches!(kind, Ownership::Current) {
        checkpoint("current.created")?;
        #[cfg(test)]
        if FAULT.with(|f| *f.borrow() == Some("current.partial")) {
            file.write_all(b"{")?;
            checkpoint("current.partial")?;
        }
        checkpoint("current.write")?;
    }
    write(&mut file)?;
    if matches!(kind, Ownership::Current) {
        checkpoint("current.sync")?;
    }
    file.sync_all()?;
    if matches!(kind, Ownership::Current) {
        checkpoint("current.written")?;
    }
    Ok(true)
}
fn put_claim(p: &mut Projection, kind: Ownership, f: ExportFile) -> anyhow::Result<()> {
    match kind {
        Ownership::Body | Ownership::Manifest => {
            let files = if matches!(kind, Ownership::Body) {
                &mut p.owned
            } else {
                &mut p.manifests
            };
            if let Some(old) = files.iter_mut().find(|v| v.path == f.path) {
                anyhow::ensure!(
                    old.device.is_none() && old.sha256 == f.sha256 && old.bytes == f.bytes,
                    "existing inode cannot be adopted"
                );
                *old = f;
            } else {
                files.push(f);
            }
        }
        Ownership::Current => {
            anyhow::ensure!(
                p.current.is_none()
                    && p.reserved_current
                        .as_ref()
                        .is_none_or(|old| old.device.is_none()
                            && old.path == f.path
                            && old.sha256 == f.sha256
                            && old.bytes == f.bytes),
                "current reservation already exists"
            );
            p.reserved_current = Some(f);
        }
        Ownership::Status => {
            anyhow::ensure!(
                p.status.as_ref().is_none_or(|old| old.device.is_none()
                    && old.path == f.path
                    && old.sha256 == f.sha256
                    && old.bytes == f.bytes),
                "status reservation already exists"
            );
            p.status = Some(f);
        }
    }
    Ok(())
}

fn verify_dir(path: &Path) -> anyhow::Result<()> {
    let m = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        m.is_dir()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o077 == 0
            && fs::canonicalize(path)? == path,
        "onboarding original directory unavailable"
    );
    Ok(())
}
fn verify_parent(root: &Path, path: &Path) -> anyhow::Result<()> {
    verify_dir(root)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("export parent missing"))?;
    let mut cursor = root.to_path_buf();
    for part in parent.strip_prefix(root)? {
        cursor.push(part);
        verify_dir(&cursor)?;
    }
    Ok(())
}
fn absent(root: &Path, path: &str) -> anyhow::Result<bool> {
    anyhow::ensure!(clean_name(path), "export path invalid");
    verify_dir(root)?;
    let mut cursor = root.to_path_buf();
    let parts: Vec<_> = Path::new(path).components().collect();
    for (index, part) in parts.iter().enumerate() {
        cursor.push(part.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(e) => return Err(e.into()),
            Ok(_) if index + 1 < parts.len() => verify_dir(&cursor)?,
            Ok(_) => return Ok(false), // dangling links exist; never settled as absent
        }
    }
    Ok(false)
}
fn remove_owned(root: &Path, file: &ExportFile) -> anyhow::Result<bool> {
    if matches(root, file)? {
        checkpoint("remove")?;
        #[cfg(test)]
        if UNLINK_FAULT_PATH.with(|p| p.borrow().as_deref() == Some(file.path.as_str())) {
            anyhow::bail!("controlled selected original unlink failure: {}", file.path);
        }
        fs::remove_file(root.join(&file.path))?;
        return Ok(true);
    }
    absent(root, &file.path)
}
fn retain_slot(
    c: &rusqlite::Connection,
    root: &Path,
    p: &mut Projection,
    reserved: bool,
) -> anyhow::Result<()> {
    let f = if reserved {
        p.reserved_current.clone()
    } else {
        p.retired_current.clone()
    };
    let Some(f) = f else { return Ok(()) };
    let result = remove_owned(root, &f);
    if !matches!(&result, Ok(true)) {
        // Keep the dedicated slot on capacity or IO failure. Revocation never
        // needs an append, even when history is already at its exact bound.
        if result.is_err() {
            return result.map(|_| ());
        }
        let mut next = p.clone();
        if !next.owned.iter().any(|old| old.path == f.path) {
            next.owned.push(f);
        }
        if reserved {
            next.reserved_current = None;
        } else {
            next.retired_current = None;
        }
        save(c, &next)?;
        *p = next;
    } else {
        if reserved {
            p.reserved_current = None;
        } else {
            p.retired_current = None;
        }
        save(c, p)?;
    }
    Ok(())
}
fn stop_current(
    c: &rusqlite::Connection,
    root: &Path,
    p: &mut Projection,
    hard: bool,
) -> anyhow::Result<()> {
    p.state = if hard { "cleanupPending" } else { "inactive" }.into();
    p.generation = None;
    anyhow::ensure!(
        p.current.is_none(),
        "current must be durably revoked before cleanup"
    );
    save(c, p)?;
    if !hard {
        retain_slot(c, root, p, true)?;
        return retain_slot(c, root, p, false);
    }
    // Never drain/take outstanding ownership before IO. Settle this bounded
    // walk in one Core write: interruption retains the original receipt even
    // for already removed files, whose real absence is safely checked on retry.
    let files: Vec<_> = p
        .owned
        .iter()
        .chain(&p.manifests)
        .chain(p.retired_current.iter())
        .chain(p.reserved_current.iter())
        .chain(p.status.iter())
        .cloned()
        .collect();
    let mut failed = false;
    for file in files {
        match remove_owned(root, &file) {
            Ok(true) => {
                p.owned.retain(|f| f.path != file.path);
                p.manifests.retain(|f| f.path != file.path);
                if p.retired_current
                    .as_ref()
                    .is_some_and(|f| f.path == file.path)
                {
                    p.retired_current = None;
                }
                if p.reserved_current
                    .as_ref()
                    .is_some_and(|f| f.path == file.path)
                {
                    p.reserved_current = None;
                }
                if p.status.as_ref().is_some_and(|f| f.path == file.path) {
                    p.status = None;
                }
            }
            Ok(false) => (),
            Err(_) => {
                failed = true;
                break; // retain the failed and every unvisited original obligation
            }
        }
    }
    let remaining = !p.owned.is_empty()
        || !p.manifests.is_empty()
        || p.retired_current.is_some()
        || p.reserved_current.is_some()
        || p.status.is_some();
    p.state = if remaining || failed {
        "cleanupUnverified"
    } else {
        "cleared"
    }
    .into();
    save(c, p)?;
    anyhow::ensure!(
        !remaining && !failed,
        "Core deletion committed; unverified exports retained for cleanup"
    );
    Ok(())
}
/// Relevant committed facts only. All reads use the current Core/Knowledge
/// authority; the projection metadata is an ownership receipt, not business data.
pub(crate) fn project_committed(c: &rusqlite::Connection, data: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        c.is_autocommit(),
        "onboarding projection requires the original post-commit boundary"
    );
    invalidate_existing(c)?;
    let result = project_inner(c, data);
    if result.is_err() {
        // Read the durable state, never a local possibly-uncommitted publish.
        invalidate_existing(c)?;
        if let Some(mut p) = load(c)? {
            if p.state == "cleanupPending" {
                p.state = "cleanupUnverified".into();
            }
            save(c, &p)?;
        }
    }
    // A human-readable private status projection is separate from the business
    // mutation result. Even a failed rebuild keeps its durable Core status.
    let was_ready = load(c)?.is_some_and(|p| p.state == "ready");
    if let Err(error) = reflect_status(c, data) {
        eprintln!(
            "crewd: onboarding status projection unavailable; durable reconciliation status retained: {error}"
        );
        // A diagnostic-file IO failure alone is best effort. If its complete
        // closure check actually revoked publication, report derived not-ready
        // rather than returning success for a now-invalidated publication.
        if error.is::<StatusQualificationFailure>()
            || (was_ready && !load(c)?.is_some_and(|p| p.state == "ready" && p.current.is_some()))
        {
            return Err(error);
        }
    }
    result
}
fn known_path(p: &Projection, path: &str) -> bool {
    p.owned
        .iter()
        .chain(&p.manifests)
        .chain(p.current.iter())
        .chain(p.status.iter())
        .chain(p.retired_current.iter())
        .chain(p.reserved_current.iter())
        .any(|f| f.path == path)
}
fn fresh_path(root: &Path, p: &Projection, canonical: &str) -> anyhow::Result<String> {
    if !known_path(p, canonical) && absent(root, canonical)? {
        return Ok(canonical.into());
    }
    let path = Path::new(canonical);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("export stem invalid"))?;
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("data");
    Ok(path
        .with_file_name(format!("{stem}-{}.{ext}", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned())
}
fn record(path: &str, digest: &str, bytes: u64) -> ExportFile {
    ExportFile {
        path: path.into(),
        sha256: digest.into(),
        bytes,
        device: None,
        inode: None,
    }
}
fn collect_body(
    k: &Knowledge,
    context: &DataContext,
    page: &facts::Page,
) -> anyhow::Result<Option<(BodyProjection, ExportPage)>> {
    let r = DataRef {
        namespace: context.namespace.clone(),
        kind: "view".into(),
        id: page.view.clone(),
    };
    let doc = k.get(&r, None)?;
    let DataObject::View {
        id,
        revision,
        title,
        definition: DataViewDefinition::Document { placements, .. },
        archived,
        ..
    } = doc.object
    else {
        anyhow::bail!("onboarding document no longer exportable")
    };
    if archived {
        return Ok(None);
    }
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut total_bytes = 0u64;
    let mut sources = Vec::new();
    let mut blocks = Vec::new();
    for placement in placements {
        let DataRefState::Visible { reference } = placement.block else {
            anyhow::bail!("onboarding placement unavailable")
        };
        anyhow::ensure!(
            reference.namespace == context.namespace && reference.kind == "block",
            "onboarding placement crosses authority"
        );
        let stored = k.get(&reference, placement.at_revision.as_deref())?;
        let DataObject::Block {
            id,
            revision,
            content_type,
            content: Value::String(part),
            archived: false,
            ..
        } = stored.object
        else {
            anyhow::bail!("onboarding current block unavailable")
        };
        anyhow::ensure!(
            content_type == "text/plain",
            "onboarding body requires normal plain-text document projection"
        );
        anyhow::ensure!(
            id == reference.id,
            "onboarding original block identity differs"
        );
        digest.update(part.as_bytes());
        total_bytes = total_bytes
            .checked_add(part.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("onboarding complete export byte count exhausted"))?;
        sources.push((reference, revision.clone()));
        blocks.push((id, revision));
    }
    let digest: String = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    anyhow::ensure!(id == page.view, "onboarding view identity differs");
    Ok(Some((
        BodyProjection {
            file: page.asset.file.clone(),
            path: String::new(),
            digest: digest.clone(),
            bytes: total_bytes,
            blocks: sources,
        },
        ExportPage {
            file: page.asset.file.clone(),
            export_path: String::new(),
            view_id: id,
            view_revision: revision,
            title,
            blocks,
            bytes: total_bytes,
            sha256: digest,
        },
    )))
}
fn current_context(
    c: &rusqlite::Connection,
    data: &Path,
    db: &str,
) -> anyhow::Result<(Knowledge, DataContext)> {
    anyhow::ensure!(
        crewd_store::meta_get(c, "dbId")?.as_deref() == Some(db),
        "original Core export authority differs"
    );
    let k = Knowledge::open(
        &data.join("knowledge").join(format!(
            "{}.sqlite",
            crewd_store::team_data::namespace(db, facts::ROOT)
        )),
        base(db),
        false,
    )?;
    checkpoint("knowledge.context")?;
    let context = k.context()?;
    anyhow::ensure!(
        context.db_id == db
            && context.authority_id == db
            && context.crew_id == facts::ROOT
            && context.namespace == crewd_store::team_data::namespace(db, facts::ROOT)
            && !context.restore_epoch.is_empty()
            && context.generation.parse::<u64>().is_ok(),
        "onboarding export authority differs"
    );
    Ok((k, context))
}
fn linked(
    c: &rusqlite::Connection,
    context: &DataContext,
    page: &facts::Page,
) -> anyhow::Result<bool> {
    let links =
        crewd_store::knowledge_channels::rows(c, &crewd_store::channels::lobby_id(facts::ROOT))?;
    Ok(links.iter().any(|l| {
        l.context.db_id == context.db_id
            && l.context.authority_id == context.authority_id
            && l.context.crew_id == context.crew_id
            && l.context.restore_epoch == context.restore_epoch
            && l.reference.namespace == context.namespace
            && l.reference.kind == "view"
            && l.reference.id == page.view
    }))
}
fn read_owned(root: &Path, f: &ExportFile) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        f.bytes <= EXPORT_BYTES_LIMIT as u64 && matches(root, f)?,
        "onboarding metadata bytes unavailable"
    );
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(&f.path))?;
    let m = file.metadata()?;
    anyhow::ensure!(
        Some(m.dev()) == f.device && Some(m.ino()) == f.inode && m.is_file() && m.nlink() == 1,
        "original metadata inode changed"
    );
    let mut bytes = Vec::new();
    file.take(
        f.bytes
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("metadata bytes exhausted"))?,
    )
    .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 == f.bytes && sha(&bytes) == f.sha256,
        "onboarding metadata changed while read"
    );
    Ok(bytes)
}
/// Validate the entire selected original closure, even at the same generation.
/// Root same-name historical copies may have user edits and are not this closure.
fn qualified(c: &rusqlite::Connection, data: &Path, p: &Projection) -> anyhow::Result<bool> {
    if p.state != "ready"
        || p.current.is_none()
        || p.reserved_current.is_some()
        || p.retired_current.is_some()
    {
        return Ok(false);
    }
    let Some(state) = facts::read(c)? else {
        return Ok(false);
    };
    if state.skipped
        || state.pending.is_some()
        || facts::root_lifecycle(c)? != facts::RootLifecycle::Active
    {
        return Ok(false);
    }
    let root = data.join("welcome-crew");
    let current = p.current.as_ref().expect("checked above");
    let pointer: Value = serde_json::from_slice(&read_owned(&root, current)?)?;
    if pointer["format"] != 1
        || pointer["readOnlyProjection"] != true
        || pointer["generation"].as_str() != p.generation.as_deref()
    {
        return Ok(false);
    }
    let Some(m) = p.manifests.iter().find(|m| {
        Some(m.path.as_str()) == pointer["manifest"].as_str()
            && Some(m.sha256.as_str()) == pointer["manifestSHA256"].as_str()
    }) else {
        return Ok(false);
    };
    let manifest: Manifest = serde_json::from_slice(&read_owned(&root, m)?)?;
    let (k, context) = current_context(c, data, &state.db_id)?;
    if manifest.format != 1
        || manifest.db_id != context.db_id
        || manifest.crew_id != context.crew_id
        || manifest.namespace != context.namespace
        || manifest.restore_epoch != context.restore_epoch
        || manifest.generation != context.generation
        || p.generation.as_deref() != Some(context.generation.as_str())
    {
        return Ok(false);
    }
    let pages: Vec<_> = state.ready.iter().flat_map(|p| &p.pages).collect();
    if pages.len() != manifest.pages.len() {
        return Ok(false);
    }
    for (page, actual) in pages.iter().zip(&manifest.pages) {
        if !linked(c, &context, page)? {
            return Ok(false);
        }
        let Some((_, mut expected)) = collect_body(&k, &context, page)? else {
            return Ok(false);
        };
        expected.export_path = actual.export_path.clone();
        if serde_json::to_value(&expected)? != serde_json::to_value(actual)? {
            return Ok(false);
        }
        let Some(body) = p.owned.iter().find(|f| {
            f.path == actual.export_path && f.sha256 == actual.sha256 && f.bytes == actual.bytes
        }) else {
            return Ok(false);
        };
        if !matches(&root, body)? {
            return Ok(false);
        }
    }
    Ok(true)
}
#[derive(Debug, thiserror::Error)]
#[error("onboarding STATUS qualification failed: {detail}")]
struct StatusQualificationFailure {
    detail: String,
    revocation_unknown: bool,
}
fn status_qualification_failure(c: &rusqlite::Connection, cause: anyhow::Error) -> anyhow::Error {
    // A failure to commit revocation is explicitly unknown. It is never
    // downgraded to harmless diagnostic IO because Core still reads ready.
    let (detail, revocation_unknown) = match invalidate_existing(c) {
        Ok(()) => (format!("trusted publication revoked; {cause:#}"), false),
        Err(error) => (
            format!(
                "readiness unknown; trusted revocation could not commit: {error:#}; original qualification failure: {cause:#}"
            ),
            true,
        ),
    };
    StatusQualificationFailure {
        detail,
        revocation_unknown,
    }
    .into()
}
fn qualify_status(c: &rusqlite::Connection, data: &Path) -> anyhow::Result<()> {
    let Some(p) = load(c)? else { return Ok(()) };
    checkpoint("status.qualify")?;
    let root = data.join("welcome-crew");
    if p.state == "ready" {
        verify_dir(data)?;
        verify_dir(&root)?;
        anyhow::ensure!(
            qualified(c, data, &p)?,
            "current closure unavailable at STATUS boundary"
        );
    } else {
        secure_dir(&root)?;
    }
    Ok(())
}
fn reflect_status(c: &rusqlite::Connection, data: &Path) -> anyhow::Result<()> {
    #[cfg(test)]
    if let Some(action) = STATUS_ACTION.with(|slot| slot.borrow_mut().take()) {
        action(c, data); // only this stage, after the inner publication commit
    }
    if let Err(error) = qualify_status(c, data) {
        return Err(status_qualification_failure(c, error));
    }
    let result = write_status(c, data);
    // Qualify after either successful or failed diagnostic IO. Only a pure
    // diagnostic error with a still-qualified closure is best effort; observed
    // ancestry/closure errors must always revoke the trusted publication.
    if let Err(error) = qualify_status(c, data) {
        return Err(status_qualification_failure(c, error));
    }
    result
}
fn write_status(c: &rusqlite::Connection, data: &Path) -> anyhow::Result<()> {
    let Some(mut p) = load(c)? else { return Ok(()) };
    let root = data.join("welcome-crew");
    checkpoint("status.write")?;
    let bytes = serde_json::to_vec_pretty(
        &json!({"format":1,"state":p.state,"generation":p.generation,
        "currentManifestPointer":if p.state == "ready" { p.current.as_ref().map(|f| &f.path) } else { None },
        "unverifiedRetainedFiles":if p.state == "cleanupUnverified" { p.owned.len()+p.manifests.len()+usize::from(p.retired_current.is_some())+usize::from(p.reserved_current.is_some()) } else { 0 },
        "meaning":"Core documents remain authoritative; non-ready status never grants a current export"}),
    )?;
    if p.status
        .as_ref()
        .is_some_and(|f| f.sha256 == sha(&bytes) && matches(&root, f).unwrap_or(false))
    {
        return Ok(());
    }
    if let Some(old) = p.status.clone() {
        let removed = remove_owned(&root, &old)?;
        let mut next = p.clone();
        if !removed && !next.owned.iter().any(|f| f.path == old.path) {
            next.owned.push(old);
        }
        next.status = None;
        save(c, &next)?;
        p = next;
    }
    let path = fresh_path(
        &root,
        &p,
        &format!("STATUS-{}-{}.json", p.state, sha(&bytes)),
    )?;
    publish(c, &root, &path, &bytes, &mut p, Ownership::Status)?;
    Ok(())
}
fn project_inner(c: &rusqlite::Connection, data: &Path) -> anyhow::Result<()> {
    let Some(state) = facts::read(c)? else {
        return Ok(());
    };
    if state.skipped || state.ready.is_empty() || state.pending.is_some() {
        return Ok(());
    }
    let mut p = load(c)?.unwrap_or_else(|| Projection {
        format: 1,
        db_id: state.db_id.clone(),
        state: "pending".into(),
        generation: None,
        owned: vec![],
        manifests: vec![],
        current: None,
        status: None,
        retired_current: None,
        reserved_current: None,
    });
    // Initial publication also persists pending before any external IO.
    save(c, &p)?;
    let root = data.join("welcome-crew");
    let lifecycle = facts::root_lifecycle(c)?;
    if lifecycle == facts::RootLifecycle::Missing {
        p.state = "cleanupPending".into();
        save(c, &p)?;
        verify_dir(data)?;
        match fs::symlink_metadata(&root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                p.owned.clear();
                p.manifests.clear();
                p.status = None;
                p.retired_current = None;
                p.reserved_current = None;
                p.state = "cleared".into();
                return save(c, &p);
            }
            Err(e) => return Err(e.into()),
            Ok(_) => verify_dir(&root)?,
        }
        return stop_current(c, &root, &mut p, true);
    }
    verify_dir(data)?;
    secure_dir(&root)?;
    if lifecycle == facts::RootLifecycle::Removed {
        return stop_current(c, &root, &mut p, false);
    }
    let (k, context) = current_context(c, data, &state.db_id)?;
    let pages: Vec<_> = state.ready.iter().flat_map(|p| &p.pages).collect();
    let mut bodies: Vec<BodyProjection> = Vec::new();
    let mut summaries = Vec::new();
    for page in pages {
        if !linked(c, &context, page)? {
            return stop_current(c, &root, &mut p, false);
        }
        let Some((mut body, mut summary)) = collect_body(&k, &context, page)? else {
            return stop_current(c, &root, &mut p, false);
        };
        let reusable = p.owned.iter().find(|f| {
            f.sha256 == body.digest
                && f.bytes == body.bytes
                && f.path.starts_with("body-")
                && matches(&root, f).unwrap_or(false)
        });
        body.path = if let Some(f) = reusable {
            f.path.clone()
        } else if let Some(f) = bodies
            .iter()
            .find(|f| f.digest == body.digest && f.bytes == body.bytes)
        {
            f.path.clone()
        } else {
            fresh_path(&root, &p, &format!("body-{}.md", body.digest))?
        };
        summary.export_path = body.path.clone();
        bodies.push(body);
        summaries.push(summary);
    }
    let manifest = Manifest {
        format: 1,
        db_id: state.db_id.clone(),
        crew_id: facts::ROOT.into(),
        namespace: context.namespace.clone(),
        restore_epoch: context.restore_epoch.clone(),
        generation: context.generation.clone(),
        historical_bundle_source:
            "e427fb2 (2026-09-30); CORE-guide-v1.md is the current Core guide".into(),
        pages: summaries,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    let digest = sha(&bytes);
    let old_manifest = p
        .manifests
        .iter()
        .find(|f| {
            f.sha256 == digest
                && f.bytes == bytes.len() as u64
                && matches(&root, f).unwrap_or(false)
        })
        .cloned();
    let manifest_file = if let Some(f) = old_manifest {
        f
    } else {
        let path = fresh_path(&root, &p, &format!("version-{digest}/manifest.json"))?;
        record(&path, &digest, bytes.len() as u64)
    };
    let pointer = serde_json::to_vec_pretty(
        &json!({"format":1,"manifest":manifest_file.path,"manifestSHA256":manifest_file.sha256,"generation":context.generation,"readOnlyProjection":true}),
    )?;
    let reuse_current = p
        .retired_current
        .as_ref()
        .filter(|f| {
            f.sha256 == sha(&pointer)
                && f.bytes == pointer.len() as u64
                && matches(&root, f).unwrap_or(false)
        })
        .cloned();
    let current_path = if let Some(f) = &reuse_current {
        f.path.clone()
    } else {
        fresh_path(
            &root,
            &p,
            &format!("current-{}-{}.json", context.generation, sha(&pointer)),
        )?
    };
    // Check the whole write batch before creating/reserving any file. Budget
    // worst-case unknown old current/reservation/status instead of dropping it.
    let mut prospective = p.clone();
    for body in &bodies {
        if !known_path(&prospective, &body.path) {
            prospective
                .owned
                .push(record(&body.path, &body.digest, body.bytes));
        }
        if !known_path(&prospective, &body.file) && absent(&root, &body.file)? {
            prospective
                .owned
                .push(record(&body.file, &body.digest, body.bytes));
        }
    }
    if !known_path(&prospective, &manifest_file.path) {
        prospective.manifests.push(manifest_file.clone());
    }
    for f in [
        p.reserved_current.as_ref(),
        if reuse_current.is_none() {
            p.retired_current.as_ref()
        } else {
            None
        },
        p.status.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if !prospective.owned.iter().any(|v| v.path == f.path) {
            prospective.owned.push(f.clone());
        }
    }
    // The reserved_current slot is independently bounded; never overload current.
    prospective.reserved_current =
        Some(record(&current_path, &sha(&pointer), pointer.len() as u64));
    for f in prospective
        .owned
        .iter_mut()
        .chain(&mut prospective.manifests)
        .chain(prospective.reserved_current.iter_mut())
    {
        if f.device.is_none() {
            f.device = Some(u64::MAX);
            f.inode = Some(u64::MAX);
        }
    }
    validate_projection(c, &prospective)?;
    retain_slot(c, &root, &mut p, true)?; // interrupted current is never promoted
    if reuse_current.is_none() {
        retain_slot(c, &root, &mut p, false)?;
    }
    // Persist planned paths without pretending their inode was acquired.
    for body in &bodies {
        if !known_path(&p, &body.path) {
            p.owned.push(record(&body.path, &body.digest, body.bytes));
        }
    }
    if !known_path(&p, &manifest_file.path) {
        p.manifests.push(manifest_file.clone());
    }
    save(c, &p)?;
    for body in &bodies {
        let existing = p
            .owned
            .iter()
            .find(|f| f.path == body.path)
            .cloned()
            .expect("planned body");
        if !matches(&root, &existing)? {
            publish_stream(
                c,
                &root,
                &body.path,
                &body.digest,
                body.bytes,
                &mut p,
                Ownership::Body,
                |file| write_body(&k, &body.blocks, file),
            )?;
        }
        let actual = p
            .owned
            .iter()
            .find(|f| f.path == body.path)
            .expect("body inode receipt");
        anyhow::ensure!(
            matches(&root, actual)?,
            "body changed or interrupted; original retained"
        );
        if !known_path(&p, &body.file) && absent(&root, &body.file)? {
            publish_stream(
                c,
                &root,
                &body.file,
                &body.digest,
                body.bytes,
                &mut p,
                Ownership::Body,
                |file| write_body(&k, &body.blocks, file),
            )?;
        }
    }
    verify_parent(&root, &root.join(&manifest_file.path)).or_else(|_| {
        secure_dir(
            root.join(&manifest_file.path)
                .parent()
                .expect("planned manifest parent"),
        )
    })?;
    let actual = p
        .manifests
        .iter()
        .find(|f| f.path == manifest_file.path)
        .cloned()
        .expect("planned manifest");
    if !matches(&root, &actual)? {
        publish(
            c,
            &root,
            &manifest_file.path,
            &bytes,
            &mut p,
            Ownership::Manifest,
        )?;
    }
    if let Some(f) = reuse_current {
        p.retired_current = None;
        p.current = Some(f);
    } else {
        publish(
            c,
            &root,
            &current_path,
            &pointer,
            &mut p,
            Ownership::Current,
        )?;
        p.current = p.reserved_current.take();
    }
    p.state = "ready".into();
    p.generation = Some(context.generation);
    anyhow::ensure!(
        qualified(c, data, &p)?,
        "complete current export closure unavailable"
    );
    checkpoint("current.commit")?;
    save(c, &p)?;
    Ok(())
}

pub(crate) fn relevant(changes: &[collab::Change]) -> bool {
    changes.iter().any(|change| match change {
        collab::Change::DataChanged { crew_id }
        | collab::Change::CrewChanged { crew_id }
        | collab::Change::CrewRemoved { crew_id } => {
            facts::CREWS.iter().any(|(id, _, _)| *id == crew_id)
        }
        collab::Change::ChannelWorkChanged { channel_id, .. }
        | collab::Change::ChannelChanged { channel_id }
        | collab::Change::ChannelRemoved { channel_id } => facts::CREWS
            .iter()
            .any(|(id, _, _)| crewd_store::channels::lobby_id(id) == *channel_id),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::Crew;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;
    pub(super) fn fresh() -> (tempfile::TempDir, PathBuf, std::fs::File, Store) {
        let t = tempfile::tempdir().unwrap();
        let data = fs::canonicalize(t.path()).unwrap();
        crate::ensure_data_dir(&data).unwrap();
        let lock = crate::lock_data_dir(&data).unwrap();
        let store = Store::open_data_dir(&data).unwrap();
        (t, data, lock, store)
    }
    pub(super) fn state(s: &Store) -> facts::State {
        facts::read(s.conn()).unwrap().unwrap()
    }
    pub(super) fn knowledge(s: &Store, data: &Path) -> Knowledge {
        let state = state(s);
        Knowledge::open(
            &data.join("knowledge").join(format!(
                "{}.sqlite",
                crewd_store::team_data::namespace(&state.db_id, facts::ROOT)
            )),
            base(&state.db_id),
            false,
        )
        .unwrap()
    }
    pub(super) fn projection(s: &Store) -> Projection {
        serde_json::from_str(
            &crewd_store::meta_get(s.conn(), EXPORT_KEY)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }
    pub(super) fn no_automation(s: &Store) {
        for table in ["sessions", "messages", "wakes"] {
            let count: i64 = s
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "unexpected {table}");
        }
    }
    /// Satisfy the ordinary non-cascading lifecycle through the actual Collab
    /// operations. This library fixture does not emit Hub changes or invoke the
    /// protected filesystem purge; each caller keeps its original projection
    /// failure cut and Core-only purge below. The service test uses public RPC.
    pub(super) fn archive_welcome_root(s: &mut Store, now: i64) {
        collab::run(s, now, |tx| {
            let shared_before = collab::crews::get(tx.conn(), "pendingcrew-welcome-child")?;
            for &(id, _, parents) in facts::CREWS {
                if !parents.contains(&facts::ROOT) {
                    continue;
                }
                let before = collab::crews::get(tx.conn(), id)?;
                assert!(before.parent_ids.iter().any(|p| p == facts::ROOT));
                let expected: Vec<_> = before
                    .parent_ids
                    .iter()
                    .filter(|p| p.as_str() != facts::ROOT)
                    .cloned()
                    .collect();
                let released = collab::crews::release(tx, id, facts::ROOT, None)?;
                assert_eq!(released.id, before.id);
                assert_eq!(released.title, before.title);
                assert_eq!(released.number, before.number);
                assert_eq!(released.parent_ids, expected);
                assert!(released.removed_at.is_none());
            }
            let shared_after = collab::crews::get(tx.conn(), "pendingcrew-welcome-child")?;
            assert_eq!(shared_after.id, shared_before.id);
            assert_eq!(shared_after.number, shared_before.number);
            assert_eq!(shared_after.parent_ids, shared_before.parent_ids);
            assert!(shared_after.removed_at.is_none());
            let root = collab::crews::archive(tx, facts::ROOT)?;
            assert!(root.child_ids.is_empty());
            assert!(root.removed_at.is_some());
            collab::crews::check_purgeable(tx.conn(), facts::ROOT)?;
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn ordinary_fresh_origin_publishes_original_dag_knowledge_and_offline_pages_without_models() {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let original = state(&s);
        assert_eq!(original.ready.len(), 1);
        assert!(original.pending.is_none());
        for (id, title, parents) in facts::CREWS {
            let c = crewd_store::crews::get(s.conn(), id).unwrap().unwrap();
            assert_eq!(&c.title, title);
            assert_eq!(
                c.parent_ids,
                parents.iter().map(|p| p.to_string()).collect::<Vec<_>>()
            );
            assert_eq!(c.working_directory, None);
            assert_eq!(c.captain_session_id, None);
        }
        assert_eq!(crewd_store::crews::list(s.conn()).unwrap().len(), 5);
        assert_eq!(
            crewd_store::knowledge_channels::rows(
                s.conn(),
                &crewd_store::channels::lobby_id(facts::ROOT)
            )
            .unwrap()
            .len(),
            BUNDLED.len()
        );
        for (name, body) in BUNDLED {
            assert_eq!(
                fs::read_to_string(data.join("welcome-crew").join(name)).unwrap(),
                *body
            );
        }
        let original_ids = serde_json::to_value(&original).unwrap();
        bootstrap(&mut s, &data).unwrap();
        assert_eq!(serde_json::to_value(state(&s)).unwrap(), original_ids);
        assert_eq!(projection(&s).state, "ready");
        no_automation(&s);
    }
    #[test]
    fn open_after_identity_before_server_hook_crash_retains_genuine_fresh_origin() {
        let (_t, data, _lock, s) = fresh();
        assert_eq!(
            crewd_store::meta_get(s.conn(), facts::FRESH_KEY)
                .unwrap()
                .as_deref(),
            Some("1")
        );
        drop(s);
        let mut s = Store::open_data_dir(&data).unwrap();
        bootstrap(&mut s, &data).unwrap();
        assert_eq!(state(&s).ready.len(), 1);
        no_automation(&s);
    }
    #[test]
    fn existing_empty_and_existing_user_organizations_are_not_injected() {
        for with_user in [false, true] {
            let t = tempfile::tempdir().unwrap();
            let data = fs::canonicalize(t.path()).unwrap();
            crate::ensure_data_dir(&data).unwrap();
            let _lock = crate::lock_data_dir(&data).unwrap();
            let mut old = Store::open(&data.join(crewd_store::DB_FILE)).unwrap();
            if with_user {
                collab::run(&mut old, 100, |tx| {
                    crewd_store::crews::upsert(
                        tx.conn(),
                        &Crew {
                            id: "existing-user".into(),
                            title: "用户改名".into(),
                            created_at: "t".into(),
                            updated_at: "t".into(),
                            ..Default::default()
                        },
                        None,
                    )?;
                    Ok(())
                })
                .unwrap();
            }
            drop(old);
            let mut s = Store::open_data_dir(&data).unwrap();
            bootstrap(&mut s, &data).unwrap();
            assert!(state(&s).skipped);
            assert_eq!(
                crewd_store::crews::list(s.conn()).unwrap().len(),
                usize::from(with_user)
            );
            assert!(!data.join("welcome-crew").exists());
        }
    }
    #[test]
    fn committed_knowledge_before_core_publication_resumes_same_original_receipt() {
        let (_t, data, _lock, mut s) = fresh();
        let (st, _) = collab::run(&mut s, 100, facts::begin).unwrap();
        let origin = base(&st.db_id);
        let (st, _) = collab::run(&mut s, 100, |tx| {
            facts::prepare(tx, EDITION, &assets(), origin)
        })
        .unwrap();
        let p = st.pending.unwrap();
        let root = data.join("knowledge");
        crewd_store::team_data::private_dir(&root).unwrap();
        let mut k = Knowledge::open(
            &root.join(format!("{}.sqlite", p.origin.namespace)),
            base(&st.db_id),
            true,
        )
        .unwrap();
        let receipt = k
            .apply(
                &p.request,
                &p.origin,
                facts::PRINCIPAL,
                &p.payload_sha256,
                BTreeMap::new(),
                mutations(&p).unwrap(),
                false,
                None,
            )
            .unwrap();
        drop(k);
        assert!(crewd_store::crews::list(s.conn()).unwrap().is_empty());
        drop(s);
        let mut s = Store::open_data_dir(&data).unwrap();
        bootstrap(&mut s, &data).unwrap();
        let k = knowledge(&s, &data);
        let known = k
            .retry(&p.request, &p.origin, facts::PRINCIPAL, &p.payload_sha256)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(known).unwrap(),
            serde_json::to_value(receipt).unwrap()
        );
        assert_eq!(state(&s).ready[0].request, p.request);
        no_automation(&s);
    }
    #[test]
    fn missing_original_knowledge_is_retained_unknown_without_replay_or_visible_crews() {
        let (_t, data, _lock, mut s) = fresh();
        let (st, _) = collab::run(&mut s, 100, facts::begin).unwrap();
        collab::run(&mut s, 100, |tx| {
            facts::prepare(tx, EDITION, &assets(), base(&st.db_id))
        })
        .unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        assert!(state(&s).pending.is_some());
        assert!(crewd_store::crews::list(s.conn()).unwrap().is_empty());
        no_automation(&s);
    }
    #[test]
    fn foreign_namespace_or_corrupt_immutable_plan_is_not_adopted() {
        let (_t, data, _lock, mut s) = fresh();
        let (st, _) = collab::run(&mut s, 100, facts::begin).unwrap();
        let root = data.join("knowledge");
        crewd_store::team_data::private_dir(&root).unwrap();
        Knowledge::open(
            &root.join(format!(
                "{}.sqlite",
                crewd_store::team_data::namespace(&st.db_id, facts::ROOT)
            )),
            base(&st.db_id),
            true,
        )
        .unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        assert!(crewd_store::crews::list(s.conn()).unwrap().is_empty());
        crewd_store::meta_set(s.conn(), facts::KEY, "{\"format\":999}").unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        no_automation(&s);
    }
    #[test]
    fn core_publication_failure_rolls_back_entire_dag_and_resumes_only_original_knowledge() {
        let (_t, data, _lock, mut s) = fresh();
        s.conn().execute_batch("CREATE TRIGGER synthetic_onboarding_failure BEFORE INSERT ON crews WHEN NEW.id='pendingcrew-welcome-questions' BEGIN SELECT RAISE(ABORT,'synthetic whole-Core-publication fault'); END;").unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        assert!(crewd_store::crews::list(s.conn()).unwrap().is_empty());
        assert!(state(&s).pending.is_some());
        let p = state(&s).pending.unwrap();
        let k = knowledge(&s, &data);
        let receipt = k
            .retry(&p.request, &p.origin, facts::PRINCIPAL, &p.payload_sha256)
            .unwrap()
            .unwrap();
        drop(k);
        no_automation(&s);
        s.conn()
            .execute_batch("DROP TRIGGER synthetic_onboarding_failure")
            .unwrap();
        bootstrap(&mut s, &data).unwrap();
        assert_eq!(state(&s).ready[0].request, receipt.request_id);
        assert_eq!(crewd_store::crews::list(s.conn()).unwrap().len(), 5);
        no_automation(&s);
    }
    #[test]
    fn state_from_a_different_core_authority_and_bad_receipts_refuse_publication() {
        let (_t, data, _lock, mut s) = fresh();
        s.conn().execute_batch("CREATE TRIGGER synthetic_hold_C BEFORE INSERT ON crews BEGIN SELECT RAISE(ABORT,'synthetic hold C'); END;").unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        let original = crewd_store::meta_get(s.conn(), facts::KEY)
            .unwrap()
            .unwrap();
        let other = Store::open_in_memory().unwrap();
        crewd_store::meta_set(other.conn(), facts::KEY, &original).unwrap();
        assert!(facts::read(other.conn()).is_err());
        let p = state(&s).pending.unwrap();
        let k = knowledge(&s, &data);
        let raw: String = k
            .conn
            .query_row(
                "SELECT body FROM _pc_receipts WHERE id=?1",
                [&p.request],
                |r| r.get(0),
            )
            .unwrap();
        let mut value: Value = serde_json::from_str(&raw).unwrap();
        value["principal"] = json!("not-original-onboarding");
        k.conn
            .execute(
                "UPDATE _pc_receipts SET body=?1 WHERE id=?2",
                rusqlite::params![serde_json::to_string(&value).unwrap(), p.request],
            )
            .unwrap();
        drop(k);
        s.conn()
            .execute_batch("DROP TRIGGER synthetic_hold_C")
            .unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        assert!(crewd_store::crews::list(s.conn()).unwrap().is_empty());
        no_automation(&s);
    }
    #[test]
    fn core_edit_preserves_filesystem_user_edit_and_exports_complete_new_native_body() {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let page = state(&s).ready[0].pages[0].clone();
        let legacy = data.join("welcome-crew").join(&page.asset.file);
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&legacy, "用户独立改稿").unwrap();
        let mut k = knowledge(&s, &data);
        let origin = k.context().unwrap();
        let mut object = k
            .get(
                &DataRef {
                    namespace: origin.namespace.clone(),
                    kind: "block".into(),
                    id: page.block.clone(),
                },
                None,
            )
            .unwrap()
            .object;
        let complete = "新的完整原文\n".repeat(20000);
        if let DataObject::Block {
            revision, content, ..
        } = &mut object
        {
            *revision = "2".into();
            *content = Value::String(complete.clone());
        }
        k.apply(
            &uuid::Uuid::new_v4().to_string(),
            &origin,
            "human:test",
            "edit-original",
            BTreeMap::new(),
            vec![Mutation::PutObject {
                object: Box::new(object),
                expected_revision: Some("1".into()),
                captures: vec![],
            }],
            false,
            None,
        )
        .unwrap();
        drop(k);
        project_committed(s.conn(), &data).unwrap();
        assert_eq!(fs::read_to_string(legacy).unwrap(), "用户独立改稿");
        let p = projection(&s);
        let exported = p
            .owned
            .iter()
            .find(|f| f.sha256 == sha(complete.as_bytes()) && f.path.starts_with("body-"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(data.join("welcome-crew").join(&exported.path)).unwrap(),
            complete
        );
        assert_eq!(p.state, "ready");
        no_automation(&s);
    }
    #[test]
    fn removal_and_purge_do_not_resurrect_and_only_verified_system_exports_are_cleaned() {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let export = data.join("welcome-crew");
        let user = export.join("INDEX.md");
        fs::set_permissions(&user, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&user, "用户另存的保留稿").unwrap();
        archive_welcome_root(&mut s, 200);
        project_committed(s.conn(), &data).unwrap();
        assert!(projection(&s).current.is_none());
        bootstrap(&mut s, &data).unwrap();
        assert!(
            crewd_store::crews::get(s.conn(), facts::ROOT)
                .unwrap()
                .unwrap()
                .removed_at
                .is_some()
        );
        // Core-only unit boundary. Root acceptance separately uses the actual
        // protected crew_lifecycle path and normal knowledge-purge contract.
        collab::run(&mut s, 300, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
        assert!(project_committed(s.conn(), &data).is_err());
        assert_eq!(fs::read_to_string(&user).unwrap(), "用户另存的保留稿");
        assert_eq!(projection(&s).state, "cleanupUnverified");
        assert!(!export.join("README-public-v2.md").exists());
        bootstrap(&mut s, &data).unwrap();
        assert!(
            crewd_store::crews::get(s.conn(), facts::ROOT)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn namespace_restore_scope_and_filesystem_aliases_stop_current_without_following() {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let export = data.join("welcome-crew");
        let target = data.join("user-target");
        fs::write(&target, "必须保留").unwrap();
        let page = state(&s).ready[0].pages[0].clone();
        let k = knowledge(&s, &data);
        let mut context = k.context().unwrap();
        context.restore_epoch = "synthetic-new-epoch".into();
        k.conn
            .execute(
                "UPDATE _pc_meta SET value=?1 WHERE key='context'",
                [serde_json::to_string(&context).unwrap()],
            )
            .unwrap();
        drop(k);
        project_committed(s.conn(), &data).unwrap();
        assert_eq!(projection(&s).state, "inactive");
        assert!(projection(&s).current.is_none());
        let altered = export.join(&page.asset.file);
        fs::remove_file(&altered).unwrap();
        symlink(&target, &altered).unwrap();
        project_committed(s.conn(), &data).unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "必须保留");
    }
}

#[cfg(test)]
#[path = "onboarding/qa.rs"]
mod successor_tests;
