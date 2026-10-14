//! Native returned-row and full-cut projection for the separate schema15 unit.
//! WorkBook/execution/public-command receipts remain the effects authorities.
//! Only the kernel-bound Core adapter may supply verified source snapshots.
//! Append observations never establish full coverage by themselves.
use crate::{Result, StoreError};
use ring::digest::{SHA256, digest};
use rusqlite::{Connection, OptionalExtension, params};

// A legal1MiB UTF8 body can expand sixfold inside JSON escaping.
// Keep its lossless encoded row without confusing the wire-frame bound.
pub const MAX_ROW_BYTES: usize = 8 * 1_048_576;
pub const MAX_CUT_BYTES: usize = 64 * 1_048_576;
pub const MAX_CUT_ROWS: usize = 100_000;
pub const V15_DDL: &str = r#"
CREATE TABLE native_history_streams(
 history_id TEXT PRIMARY KEY,
 runtime_id TEXT NOT NULL REFERENCES native_execution_runtimes(runtime_id) ON DELETE CASCADE,
 native_session_id TEXT NOT NULL,
 sdk_sha256 TEXT NOT NULL CHECK(length(sdk_sha256)=64),
 coverage TEXT NOT NULL CHECK(coverage IN('incomplete','complete','historical')),
 cut_id TEXT,
 revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0),
 created_at TEXT NOT NULL,
 UNIQUE(runtime_id,native_session_id)
);
CREATE TRIGGER native_history_source_immutable BEFORE UPDATE OF history_id,runtime_id,native_session_id,sdk_sha256,created_at ON native_history_streams BEGIN SELECT RAISE(ABORT,'native history source immutable'); END;
CREATE TABLE native_history_rows(
 history_id TEXT NOT NULL REFERENCES native_history_streams(history_id) ON DELETE CASCADE,
 native_uuid TEXT NOT NULL,
 row_bytes BLOB NOT NULL CHECK(length(row_bytes) BETWEEN 1 AND 8388608),
 row_sha256 TEXT NOT NULL CHECK(length(row_sha256)=64),
 replay_supported INTEGER NOT NULL CHECK(replay_supported IN(0,1)),
 PRIMARY KEY(history_id,native_uuid)
);
CREATE TRIGGER native_history_row_immutable BEFORE UPDATE ON native_history_rows BEGIN SELECT RAISE(ABORT,'native history row immutable'); END;
CREATE TABLE native_history_cuts(
 cut_id TEXT PRIMARY KEY,
 history_id TEXT NOT NULL REFERENCES native_history_streams(history_id) ON DELETE CASCADE,
 source_census BLOB NOT NULL CHECK(length(source_census) BETWEEN 1 AND 8388608),
 source_sha256 TEXT NOT NULL CHECK(length(source_sha256)=64),
 row_count INTEGER NOT NULL CHECK(row_count BETWEEN 0 AND 100000),
 row_digest TEXT NOT NULL CHECK(length(row_digest)=64),
 created_at TEXT NOT NULL
);
CREATE TABLE native_history_cut_rows(
 cut_id TEXT NOT NULL REFERENCES native_history_cuts(cut_id) ON DELETE CASCADE,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0),
 history_id TEXT NOT NULL,
 native_uuid TEXT NOT NULL,
 PRIMARY KEY(cut_id,ordinal),
 UNIQUE(cut_id,native_uuid),
 FOREIGN KEY(history_id,native_uuid) REFERENCES native_history_rows(history_id,native_uuid) ON DELETE CASCADE
);
CREATE TRIGGER native_history_cut_immutable BEFORE UPDATE ON native_history_cuts BEGIN SELECT RAISE(ABORT,'native history cut immutable'); END;
CREATE TRIGGER native_history_cut_row_immutable BEFORE UPDATE ON native_history_cut_rows BEGIN SELECT RAISE(ABORT,'native history cut row immutable'); END;
CREATE INDEX native_history_runtime ON native_history_streams(runtime_id,coverage);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub history_id: String,
    pub runtime_id: String,
    pub native_session_id: String,
    pub sdk_sha256: String,
}
/// Exact authenticated returned native row, not a rendered screen.
/// Unknown media/blocks stay lossless here and prevent complete replay.
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    pub native_uuid: String,
    pub bytes: Vec<u8>,
    pub replay_supported: bool,
}
impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("native_uuid", &self.native_uuid)
            .field("bytes", &"<private>")
            .field("replay_supported", &self.replay_supported)
            .finish()
    }
}
/// This is a trusted-adapter argument, never a public/Mod JSON attestation.
/// The adapter must first verify actual owned transcript identities, stable
/// file cut, source format and every ancestor/fork/compaction row census.
/// A newest4096 result or continuous bridge sequence cannot produce it.
/// Fields are private: production can only obtain this from the forthcoming
/// owned native transcript verifier after its actual format/coverage gate.
/// No generic JSON/Mod observation constructor is exposed.
pub struct VerifiedSourceCut {
    cut_id: String,
    source_census: Vec<u8>,
    source_sha256: String,
    rows: Vec<Row>,
}
impl std::fmt::Debug for VerifiedSourceCut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedSourceCut")
            .field("cut_id", &self.cut_id)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}
fn invalid(s: &str) -> StoreError {
    StoreError::Invalid(s.into())
}
fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn uuid(value: &str) -> bool {
    crewd_proto::valid_remote_uuid(value)
}
fn validate(binding: &Binding) -> Result<()> {
    if !uuid(&binding.history_id)
        || !uuid(&binding.runtime_id)
        || uuid::Uuid::parse_str(&binding.native_session_id).is_err()
        || !sha(&binding.sdk_sha256)
    {
        return Err(invalid("native history source identity invalid"));
    }
    Ok(())
}
fn current(c: &Connection, binding: &Binding) -> Result<()> {
    validate(binding)?;
    let value: Option<(String,String,String,String)> = c.query_row("SELECT runtime_id,native_session_id,sdk_sha256,coverage FROM native_history_streams WHERE history_id=?1", [&binding.history_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((runtime, session, sdk, coverage)) = value else {
        return Err(invalid("native history stream unavailable"));
    };
    if runtime != binding.runtime_id
        || session != binding.native_session_id
        || sdk != binding.sdk_sha256
        || coverage == "historical"
    {
        return Err(invalid("native history binding changed or historical"));
    }
    let live: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_execution_runtimes WHERE runtime_id=?1 AND revoked=0)",
        [&binding.runtime_id],
        |r| r.get(0),
    )?;
    if !live {
        return Err(invalid("native history runtime revoked"));
    }
    Ok(())
}
/// Enter the history projection through the owning Store/FIFO transaction.
/// Mutable access is needed solely for nested savepoints: each history operation
/// must roll back its own failed row/cut even when the outer Core task handles
/// that error. This does not open another connection or change with_tx callers.
/// The separate schema15 migration remains required before history use.
pub fn with_transaction<T>(
    store: &mut crate::Store,
    f: impl FnOnce(&mut rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let mut tx = store.conn.transaction()?;
    let value = f(&mut tx)?;
    tx.commit()?;
    Ok(value)
}

/// Caller supplies the Core transaction. Registering an empty stream is still
/// incomplete; zero observed rows never prove a genuinely empty native source.
pub fn register(tx: &mut rusqlite::Transaction<'_>, binding: &Binding, now: &str) -> Result<()> {
    validate(binding)?;
    let savepoint = tx.savepoint()?;
    savepoint.execute("INSERT INTO native_history_streams(history_id,runtime_id,native_session_id,sdk_sha256,coverage,created_at) VALUES(?1,?2,?3,?4,'incomplete',?5)", params![binding.history_id,binding.runtime_id,binding.native_session_id,binding.sdk_sha256,now])?;
    current(&savepoint, binding)?;
    savepoint.commit()?;
    Ok(())
}
fn row(tx: &Connection, history: &str, value: &Row) -> Result<bool> {
    if uuid::Uuid::parse_str(&value.native_uuid).is_err()
        || value.bytes.is_empty()
        || value.bytes.len() > MAX_ROW_BYTES
    {
        return Err(invalid("native history row size or identity unsupported"));
    }
    let _: serde_json::Value = serde_json::from_slice(&value.bytes)?;
    let prior: Option<(Vec<u8>,bool)> = tx.query_row("SELECT row_bytes,replay_supported FROM native_history_rows WHERE history_id=?1 AND native_uuid=?2", params![history,value.native_uuid], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((bytes, supported)) = prior {
        if bytes != value.bytes || supported != value.replay_supported {
            return Err(invalid("native history UUID content conflict"));
        }
        return Ok(false);
    }
    tx.execute("INSERT INTO native_history_rows(history_id,native_uuid,row_bytes,row_sha256,replay_supported) VALUES(?1,?2,?3,?4,?5)", params![history,value.native_uuid,value.bytes,hash(&value.bytes),value.replay_supported])?;
    Ok(true)
}
/// A new observation invalidates an older complete cut. Duplicate exact native
/// UUIDs do not create a fictitious sequence, cut, count or second observation.
pub fn observe(tx: &mut rusqlite::Transaction<'_>, binding: &Binding, value: &Row) -> Result<bool> {
    let savepoint = tx.savepoint()?;
    current(&savepoint, binding)?;
    let inserted = row(&savepoint, &binding.history_id, value)?;
    if inserted {
        invalidate(&savepoint, binding)?;
    }
    savepoint.commit()?;
    Ok(inserted)
}
pub fn invalidate(tx: &Connection, binding: &Binding) -> Result<()> {
    current(tx, binding)?;
    let changed=tx.execute("UPDATE native_history_streams SET coverage='incomplete',cut_id=NULL,revision=revision+1 WHERE history_id=?1 AND revision<9223372036854775807", [&binding.history_id])?;
    if changed != 1 {
        return Err(invalid("native history revision overflow"));
    }
    Ok(())
}
/// Persist the entire source-proven cut atomically. A row parser finding unknown
/// content must mark its row unsupported; no such cut becomes complete.
/// Rows already observed but absent from this cut prove a gap/race, not coverage.
pub fn commit_cut(
    tx: &mut rusqlite::Transaction<'_>,
    binding: &Binding,
    cut: &VerifiedSourceCut,
    now: &str,
) -> Result<()> {
    let savepoint = tx.savepoint()?;
    commit_cut_inner(&savepoint, binding, cut, now)?;
    savepoint.commit()?;
    Ok(())
}
fn commit_cut_inner(
    tx: &Connection,
    binding: &Binding,
    cut: &VerifiedSourceCut,
    now: &str,
) -> Result<()> {
    current(tx, binding)?;
    if !uuid(&cut.cut_id)
        || cut.source_census.is_empty()
        || cut.source_census.len() > MAX_ROW_BYTES
        || !sha(&cut.source_sha256)
        || hash(&cut.source_census) != cut.source_sha256
        || cut.rows.len() > MAX_CUT_ROWS
    {
        return Err(invalid("native history verified source cut invalid"));
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut total = 0usize;
    let mut ordered = Vec::new();
    for value in &cut.rows {
        if !identities.insert(value.native_uuid.as_str()) || !value.replay_supported {
            return Err(invalid("native history incomplete or unsupported replay"));
        }
        total = total
            .checked_add(value.bytes.len())
            .ok_or_else(|| invalid("native history cut size overflow"))?;
        if total > MAX_CUT_BYTES {
            return Err(invalid("native history cut size unsupported"));
        }
        row(tx, &binding.history_id, value)?;
        ordered.extend_from_slice(value.native_uuid.as_bytes());
        ordered.push(0);
        ordered.extend_from_slice(hash(&value.bytes).as_bytes());
        ordered.push(10);
    }
    let known: i64 = tx.query_row(
        "SELECT COUNT(*) FROM native_history_rows WHERE history_id=?1",
        [&binding.history_id],
        |r| r.get(0),
    )?;
    if usize::try_from(known).ok() != Some(cut.rows.len()) {
        return Err(invalid(
            "native history source census omitted observed rows",
        ));
    }
    tx.execute("INSERT INTO native_history_cuts(cut_id,history_id,source_census,source_sha256,row_count,row_digest,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![cut.cut_id,binding.history_id,cut.source_census,cut.source_sha256,cut.rows.len() as i64,hash(&ordered),now])?;
    for (ordinal, value) in cut.rows.iter().enumerate() {
        tx.execute("INSERT INTO native_history_cut_rows(cut_id,ordinal,history_id,native_uuid) VALUES(?1,?2,?3,?4)", params![cut.cut_id,ordinal as i64,binding.history_id,value.native_uuid])?;
    }
    let changed=tx.execute("UPDATE native_history_streams SET coverage='complete',cut_id=?2,revision=revision+1 WHERE history_id=?1 AND revision<9223372036854775807", params![binding.history_id,cut.cut_id])?;
    if changed != 1 {
        return Err(invalid("native history revision overflow"));
    }
    Ok(())
}
/// Reads all rows in source-proven order at one DB transaction high-water.
/// ACP replay must emit these updates before session/load response; this reader
/// intentionally never truncates to the SDK's newest4096-row window.
pub fn replay(tx: &rusqlite::Transaction<'_>, binding: &Binding) -> Result<Vec<Row>> {
    current(tx, binding)?;
    let cut: Option<String> = tx
        .query_row(
            "SELECT cut_id FROM native_history_streams WHERE history_id=?1 AND coverage='complete'",
            [&binding.history_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let cut = cut.ok_or_else(|| invalid("native complete history unavailable"))?;
    let (count, expected_digest, source, source_hash): (i64,String,Vec<u8>,String) = tx.query_row(
        "SELECT row_count,row_digest,source_census,source_sha256 FROM native_history_cuts WHERE cut_id=?1 AND history_id=?2",
        params![cut, binding.history_id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    )?;
    if hash(&source) != source_hash {
        return Err(invalid("native history source cut damaged"));
    }
    let mut stmt=tx.prepare("SELECT r.native_uuid,r.row_bytes,r.replay_supported,r.row_sha256,c.ordinal FROM native_history_cut_rows c JOIN native_history_rows r ON r.history_id=c.history_id AND r.native_uuid=c.native_uuid WHERE c.cut_id=?1 ORDER BY c.ordinal")?;
    let rows = stmt
        .query_map([&cut], |r| {
            Ok((
                Row {
                    native_uuid: r.get(0)?,
                    bytes: r.get(1)?,
                    replay_supported: r.get(2)?,
                },
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if usize::try_from(count).ok() != Some(rows.len()) {
        return Err(invalid("native complete history cut inconsistent"));
    }
    let mut ordered = Vec::new();
    let mut output = Vec::with_capacity(rows.len());
    for (ordinal, (row, expected_hash, stored_ordinal)) in rows.into_iter().enumerate() {
        if !row.replay_supported
            || row.bytes.len() > MAX_ROW_BYTES
            || stored_ordinal != ordinal as i64
            || hash(&row.bytes) != expected_hash
        {
            return Err(invalid(
                "native complete history row damaged or unsupported",
            ));
        }
        ordered.extend_from_slice(row.native_uuid.as_bytes());
        ordered.push(0);
        ordered.extend_from_slice(expected_hash.as_bytes());
        ordered.push(10);
        output.push(row);
    }
    if hash(&ordered) != expected_digest {
        return Err(invalid("native complete history row census damaged"));
    }
    Ok(output)
}
/// Restart/restore/import can preserve historical raw evidence but never grant
/// current coverage from a copied cut. Fresh physical/native source must rebind.
pub fn historical(tx: &rusqlite::Transaction<'_>) -> Result<usize> {
    let overflow: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_history_streams WHERE coverage!='historical' AND revision>=9223372036854775807)", [], |r| r.get(0))?;
    if overflow {
        return Err(invalid("native history revision overflow"));
    }
    Ok(tx.execute("UPDATE native_history_streams SET coverage='historical',cut_id=NULL,revision=revision+1 WHERE coverage!='historical'", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn fixture() -> (Connection, Binding) {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE native_execution_runtimes(runtime_id TEXT PRIMARY KEY,revoked INTEGER NOT NULL);").unwrap();
        c.execute_batch(V15_DDL).unwrap();
        let binding = Binding {
            history_id: id(),
            runtime_id: id(),
            native_session_id: id(),
            sdk_sha256: hash(b"synthetic SDK projection fixture"),
        };
        c.execute(
            "INSERT INTO native_execution_runtimes VALUES(?1,0)",
            [&binding.runtime_id],
        )
        .unwrap();
        let mut tx = c.unchecked_transaction().unwrap();
        register(&mut tx, &binding, "now").unwrap();
        tx.commit().unwrap();
        (c, binding)
    }
    fn native_row(index: usize) -> Row {
        Row {
            native_uuid: id(),
            bytes: serde_json::to_vec(
                &serde_json::json!({"syntheticProjectionRow":index,"text":"完整 UTF8 🛫"}),
            )
            .unwrap(),
            replay_supported: true,
        }
    }
    fn synthetic_cut(rows: Vec<Row>) -> VerifiedSourceCut {
        let source=serde_json::to_vec(&serde_json::json!({"syntheticOnly":true,"nativeFormatProof":false,"rowCount":rows.len()})).unwrap();
        VerifiedSourceCut {
            cut_id: id(),
            source_sha256: hash(&source),
            source_census: source,
            rows,
        }
    }
    #[test]
    fn owning_store_transaction_keeps_nested_failure_and_outer_commit_atomic() {
        let (conn, binding) = fixture();
        let mut store = crate::Store {
            conn,
            backups: None,
        };
        let first = native_row(1);
        with_transaction(&mut store, |tx| {
            assert!(observe(tx, &binding, &first)?);
            // The caller intentionally handles a failed cut. Its savepoint must
            // not retain the extra row when the owning outer transaction commits.
            let extra = native_row(2);
            assert!(commit_cut(tx, &binding, &synthetic_cut(vec![extra]), "cut").is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            store
                .conn()
                .query_row("SELECT COUNT(*) FROM native_history_rows", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let second = native_row(3);
        let rejected: Result<()> = with_transaction(&mut store, |tx| {
            observe(tx, &binding, &second)?;
            Err(invalid("synthetic Core task abort"))
        });
        assert!(rejected.is_err());
        assert_eq!(
            store
                .conn()
                .query_row("SELECT COUNT(*) FROM native_history_rows", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn deleting_the_owning_runtime_removes_only_its_history_projection() {
        let (c, binding) = fixture();
        let mut tx = c.unchecked_transaction().unwrap();
        commit_cut(
            &mut tx,
            &binding,
            &synthetic_cut(vec![native_row(1)]),
            "cut",
        )
        .unwrap();
        tx.commit().unwrap();
        // Existing session/crew deletion owns runtime lifetime. Projection rows
        // cannot prevent that authority's deletion or survive as private orphans.
        c.execute(
            "DELETE FROM native_execution_runtimes WHERE runtime_id=?1",
            [&binding.runtime_id],
        )
        .unwrap();
        for table in [
            "native_history_streams",
            "native_history_rows",
            "native_history_cuts",
            "native_history_cut_rows",
        ] {
            assert_eq!(
                c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert!(
            c.prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn zero_observations_and_continuous_hooks_do_not_establish_full_coverage() {
        let (c, b) = fixture();
        let mut tx = c.unchecked_transaction().unwrap();
        assert!(replay(&tx, &b).is_err());
        for i in 0..4 {
            assert!(observe(&mut tx, &b, &native_row(i)).unwrap());
        }
        assert!(replay(&tx, &b).is_err());
        tx.commit().unwrap();
    }
    #[test]
    fn synthetic_complete_projection_above4096_replays_without_truncation() {
        let (c, b) = fixture();
        let values = (0..5000).map(native_row).collect::<Vec<_>>();
        let cut = synthetic_cut(values.clone());
        let mut tx = c.unchecked_transaction().unwrap();
        commit_cut(&mut tx, &b, &cut, "cut").unwrap();
        tx.commit().unwrap();
        let mut tx = c.unchecked_transaction().unwrap();
        assert_eq!(replay(&tx, &b).unwrap(), values);
        assert!(!observe(&mut tx, &b, &values[0]).unwrap());
        assert_eq!(replay(&tx, &b).unwrap().len(), 5000);
        observe(&mut tx, &b, &native_row(5001)).unwrap();
        assert!(replay(&tx, &b).is_err());
        tx.commit().unwrap();
    }
    #[test]
    fn unknown_media_census_gap_and_conflicting_uuid_never_become_complete() {
        let (c, b) = fixture();
        let old = native_row(1);
        let mut tx = c.unchecked_transaction().unwrap();
        observe(&mut tx, &b, &old).unwrap();
        tx.commit().unwrap();
        let mut tx = c.unchecked_transaction().unwrap();
        let mut unsupported = native_row(2);
        unsupported.replay_supported = false;
        assert!(commit_cut(&mut tx, &b, &synthetic_cut(vec![unsupported]), "cut").is_err());
        assert!(replay(&tx, &b).is_err());
        drop(tx);
        let mut tx = c.unchecked_transaction().unwrap();
        assert!(commit_cut(&mut tx, &b, &synthetic_cut(vec![native_row(3)]), "cut").is_err());
        tx.commit().unwrap();
        let mut tx = c.unchecked_transaction().unwrap();
        let mut changed = old;
        changed.bytes = b"{}".to_vec();
        assert!(observe(&mut tx, &b, &changed).is_err());
        drop(tx);
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_history_rows", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn restored_and_revoked_sources_preserve_rows_without_regranting_coverage() {
        let (c, b) = fixture();
        let mut tx = c.unchecked_transaction().unwrap();
        commit_cut(&mut tx, &b, &synthetic_cut(vec![native_row(1)]), "cut").unwrap();
        tx.commit().unwrap();
        let mut tx = c.unchecked_transaction().unwrap();
        assert_eq!(historical(&tx).unwrap(), 1);
        assert!(replay(&tx, &b).is_err());
        assert!(observe(&mut tx, &b, &native_row(2)).is_err());
        tx.commit().unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_history_rows", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let (c, b) = fixture();
        c.execute("UPDATE native_execution_runtimes SET revoked=1", [])
            .unwrap();
        let mut tx = c.unchecked_transaction().unwrap();
        assert!(observe(&mut tx, &b, &native_row(1)).is_err());
    }
    #[test]
    fn corrupted_cut_census_and_revision_overflow_fail_closed() {
        let (c, b) = fixture();
        let mut tx = c.unchecked_transaction().unwrap();
        let cut = synthetic_cut(vec![native_row(1)]);
        commit_cut(&mut tx, &b, &cut, "cut").unwrap();
        tx.commit().unwrap();
        // A deleted ordinal simulates a damaged projection, not a valid source cut.
        c.execute(
            "DELETE FROM native_history_cut_rows WHERE cut_id=?1",
            [&cut.cut_id],
        )
        .unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(replay(&tx, &b).is_err());
        drop(tx);
        c.execute(
            "UPDATE native_history_streams SET revision=9223372036854775807",
            [],
        )
        .unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(historical(&tx).is_err());
        assert!(invalidate(&tx, &b).is_err());
        drop(tx);
        let mut tx = c.unchecked_transaction().unwrap();
        assert!(observe(&mut tx, &b, &native_row(99)).is_err());
        tx.commit().unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_history_rows", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
