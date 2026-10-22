//! Internal Core execution receipts, not a provider/team work ledger. Only the
//! owning DB FIFO transaction writes these tables. Helpers never receive a writer.
//! Formal version12 follows the remote version11 migration; provider capability remains independently gated.
use crate::{Result, StoreError};
use ring::digest::{SHA256, digest};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

pub const V12_DDL: &str = r#"
CREATE TABLE native_execution_runtimes(
 runtime_id TEXT PRIMARY KEY,
 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 generation TEXT NOT NULL,
 native_epoch INTEGER NOT NULL CHECK(native_epoch>0),
 native_pid INTEGER NOT NULL CHECK(native_pid>1),
 native_start INTEGER NOT NULL CHECK(native_start>0),
 native_boot TEXT NOT NULL,
 revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN(0,1)),
 manual INTEGER NOT NULL DEFAULT 0 CHECK(manual IN(0,1)),
 revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0)
);
CREATE UNIQUE INDEX native_execution_one_runtime ON native_execution_runtimes(session_id) WHERE revoked=0;
CREATE TRIGGER native_execution_runtime_identity_immutable BEFORE UPDATE OF runtime_id,session_id,generation,native_epoch,native_pid,native_start,native_boot ON native_execution_runtimes BEGIN
 SELECT RAISE(ABORT,'native runtime identity is immutable');
END;
CREATE TABLE native_execution_receipts(
 request_id TEXT PRIMARY KEY,
 runtime_id TEXT NOT NULL REFERENCES native_execution_runtimes(runtime_id) ON DELETE CASCADE,
 phase TEXT NOT NULL CHECK(phase IN('prepared','content_accepted','running','cancel_requested','raw_revoked','unknown','completed','aborted','failed')),
 prompt_id TEXT,
 turn_id TEXT,
 revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0),
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
);
CREATE INDEX native_execution_runtime_receipts ON native_execution_receipts(runtime_id,request_id);
CREATE UNIQUE INDEX native_execution_one_open_request ON native_execution_receipts(runtime_id) WHERE phase NOT IN('completed','aborted','failed');
CREATE TRIGGER native_execution_request_identity_immutable BEFORE UPDATE OF request_id,runtime_id,created_at ON native_execution_receipts BEGIN
 SELECT RAISE(ABORT,'native request identity is immutable');
END;
CREATE TABLE native_execution_inputs(
 request_id TEXT PRIMARY KEY REFERENCES native_execution_receipts(request_id) ON DELETE CASCADE,
 body BLOB NOT NULL CHECK(length(body)>0 AND length(body)<=1048576),
 witness BLOB NOT NULL CHECK(length(witness)=32)
);
CREATE TRIGGER native_execution_input_immutable BEFORE UPDATE ON native_execution_inputs BEGIN
 SELECT RAISE(ABORT,'native original input is immutable');
END;
"#;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub id: String,
    pub session: String,
    pub generation: String,
    pub epoch: u64,
    pub native_pid: i32,
    pub native_start: u64,
    pub native_boot: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    ContentAccepted,
    Running,
    CancelRequested,
    RawRevoked,
    Unknown,
    Completed,
    Aborted,
    Failed,
}
impl Phase {
    pub fn key(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::ContentAccepted => "content_accepted",
            Self::Running => "running",
            Self::CancelRequested => "cancel_requested",
            Self::RawRevoked => "raw_revoked",
            Self::Unknown => "unknown",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }
    fn read(s: String) -> Result<Self> {
        match s.as_str() {
            "prepared" => Ok(Self::Prepared),
            "content_accepted" => Ok(Self::ContentAccepted),
            "running" => Ok(Self::Running),
            "cancel_requested" => Ok(Self::CancelRequested),
            "raw_revoked" => Ok(Self::RawRevoked),
            "unknown" => Ok(Self::Unknown),
            "completed" => Ok(Self::Completed),
            "aborted" => Ok(Self::Aborted),
            "failed" => Ok(Self::Failed),
            _ => Err(invalid("corrupt native receipt phase")),
        }
    }
    fn settled(self) -> bool {
        matches!(self, Self::Completed | Self::Aborted | Self::Failed)
    }
}
/// Metadata deliberately excludes the immutable body and SHA witness. Ordinary
/// bounded reads do not join or deserialize the inputs table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub request_id: String,
    pub runtime_id: String,
    pub phase: Phase,
    pub prompt_id: Option<String>,
    pub turn_id: Option<String>,
    pub revision: i64,
}
fn invalid(text: &'static str) -> StoreError {
    StoreError::Invalid(text.into())
}
fn uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|id| !id.is_nil() && id.to_string() == s)
}
fn valid_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn active(tx: &Transaction<'_>, runtime: &str) -> Result<(bool, i64)> {
    let state: Option<(i64, i64, i64)> = tx
        .query_row(
            "SELECT revoked,manual,revision FROM native_execution_runtimes WHERE runtime_id=?1",
            [runtime],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match state {
        Some((0, manual, revision)) => Ok((manual != 0, revision)),
        _ => Err(invalid("native runtime revoked or unknown")),
    }
}
/// Read the current immutable native birth projection. Historical/revoked
/// rows cannot become a new physical or automatic authority on reopening.
pub fn current_runtime(conn: &rusqlite::Connection, session: &str) -> Result<Option<Runtime>> {
    Ok(conn.query_row(
        "SELECT runtime_id,session_id,generation,native_epoch,native_pid,native_start,native_boot FROM native_execution_runtimes WHERE session_id=?1 AND revoked=0",
        [session], |row| Ok(Runtime { id:row.get(0)?,session:row.get(1)?,generation:row.get(2)?,epoch:u64::try_from(row.get::<_,i64>(3)?).map_err(|_|rusqlite::Error::IntegralValueOutOfRange(3,-1))?,native_pid:row.get(4)?,native_start:u64::try_from(row.get::<_,i64>(5)?).map_err(|_|rusqlite::Error::IntegralValueOutOfRange(5,-1))?,native_boot:row.get(6)? })
    ).optional()?)
}
fn affected(actual: usize, expected: usize) -> Result<()> {
    if actual != expected {
        return Err(invalid(
            "native durable mutation did not affect its exact owning rows",
        ));
    }
    Ok(())
}
fn pending(tx: &rusqlite::Connection, runtime: &str) -> Result<usize> {
    let (count,overflow):(i64,bool)=tx.query_row("SELECT COUNT(*),COALESCE(MAX(revision)>=9223372036854775807,0) FROM native_execution_receipts WHERE runtime_id=?1 AND phase NOT IN('completed','aborted','failed')",[runtime],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if overflow {
        return Err(invalid("native pending revision exhausted"));
    }
    usize::try_from(count).map_err(|_| invalid("native pending count overflow"))
}
pub fn register(tx: &Transaction<'_>, runtime: &Runtime) -> Result<()> {
    if !uuid(&runtime.id)
        || !uuid(&runtime.generation)
        || !valid_text(&runtime.session, 256)
        || runtime.native_pid <= 1
        || runtime.epoch == 0
        || runtime.native_start == 0
        || runtime.native_boot.len() > 256
    {
        return Err(invalid("invalid native runtime"));
    }
    let epoch = i64::try_from(runtime.epoch).map_err(|_| invalid("native epoch overflow"))?;
    let start =
        i64::try_from(runtime.native_start).map_err(|_| invalid("native birth overflow"))?;
    affected(tx.execute("INSERT INTO native_execution_runtimes(runtime_id,session_id,generation,native_epoch,native_pid,native_start,native_boot) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![runtime.id,runtime.session,runtime.generation,epoch,runtime.native_pid,start,runtime.native_boot])?,1)?;
    Ok(())
}
type RawReceipt = (String, String, String, Option<String>, Option<String>, i64);
pub fn get(conn: &rusqlite::Connection, request: &str) -> Result<Option<Receipt>> {
    let raw:Option<RawReceipt>=conn.query_row("SELECT request_id,runtime_id,phase,prompt_id,turn_id,revision FROM native_execution_receipts WHERE request_id=?1",[request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    raw.map(
        |(request_id, runtime_id, phase, prompt_id, turn_id, revision)| {
            Ok(Receipt {
                request_id,
                runtime_id,
                phase: Phase::read(phase)?,
                prompt_id,
                turn_id,
                revision,
            })
        },
    )
    .transpose()
}
/// Exact original request retry confirms current metadata; it neither recreates
/// an input nor restores old runtime admission. Binding/payload changes conflict.
pub fn prepare(
    tx: &Transaction<'_>,
    runtime: &str,
    request: &str,
    body: &[u8],
    now: &str,
) -> Result<(Receipt, bool)> {
    if !uuid(request)
        || body.is_empty()
        || body.len() > 1_048_576
        || std::str::from_utf8(body).is_err()
        || !valid_text(now, 128)
    {
        return Err(invalid("invalid native input"));
    }
    if let Some(existing) = get(tx, request)? {
        let original: Vec<u8> = tx.query_row(
            "SELECT body FROM native_execution_inputs WHERE request_id=?1",
            [request],
            |r| r.get(0),
        )?;
        if existing.runtime_id != runtime || original != body {
            return Err(invalid("native request payload conflict"));
        }
        return Ok((existing, true));
    }
    let (manual, _) = active(tx, runtime)?;
    if manual {
        return Err(invalid("native runtime belongs to manual input"));
    }
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM native_execution_receipts WHERE runtime_id=?1",
        [runtime],
        |r| r.get(0),
    )?;
    if count >= 64 {
        return Err(invalid("native runtime receipt bound exceeded"));
    }
    affected(tx.execute("INSERT INTO native_execution_receipts(request_id,runtime_id,phase,created_at,updated_at) VALUES(?1,?2,'prepared',?3,?3)",params![request,runtime,now])?,1)?;
    affected(
        tx.execute(
            "INSERT INTO native_execution_inputs(request_id,body,witness) VALUES(?1,?2,?3)",
            params![request, body, digest(&SHA256, body).as_ref()],
        )?,
        1,
    )?;
    Ok((
        get(tx, request)?.ok_or_else(|| invalid("native receipt insertion missing"))?,
        false,
    ))
}
/// Private on-demand original input; no Debug exposes model/user content.
pub struct OriginalInput {
    body: Vec<u8>,
}
impl OriginalInput {
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }
}
impl std::fmt::Debug for OriginalInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginalInput")
            .field("body", &"<private>")
            .finish()
    }
}
pub fn original_input(
    conn: &rusqlite::Connection,
    runtime: &str,
    request: &str,
) -> Result<Option<OriginalInput>> {
    let row:Option<(Vec<u8>,Vec<u8>)>=conn.query_row("SELECT i.body,i.witness FROM native_execution_inputs i JOIN native_execution_receipts r ON r.request_id=i.request_id WHERE r.runtime_id=?1 AND r.request_id=?2",params![runtime,request],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(body, witness)| {
        if body.is_empty()
            || body.len() > 1_048_576
            || std::str::from_utf8(&body).is_err()
            || witness != digest(&SHA256, &body).as_ref()
        {
            return Err(invalid("corrupt native original input"));
        }
        Ok(OriginalInput { body })
    })
    .transpose()
}
pub fn list_metadata(
    conn: &rusqlite::Connection,
    runtime: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Receipt>> {
    if limit == 0 || limit > 64 || after.is_some_and(|id| !uuid(id)) {
        return Err(invalid("invalid native metadata page"));
    }
    let mut statement=conn.prepare("SELECT request_id FROM native_execution_receipts WHERE runtime_id=?1 AND (?2 IS NULL OR request_id>?2) ORDER BY request_id LIMIT ?3")?;
    let ids = statement
        .query_map(params![runtime, after, limit as i64], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| get(conn, &id)?.ok_or_else(|| invalid("native metadata disappeared")))
        .collect()
}
fn owned(tx: &Transaction<'_>, runtime: &str, request: &str, revision: i64) -> Result<Receipt> {
    active(tx, runtime)?;
    let receipt = get(tx, request)?.ok_or_else(|| invalid("native receipt absent"))?;
    if receipt.runtime_id != runtime || receipt.revision != revision {
        return Err(invalid("native receipt CAS or runtime conflict"));
    }
    Ok(receipt)
}
fn update(
    tx: &Transaction<'_>,
    receipt: &Receipt,
    phase: Phase,
    prompt: Option<&str>,
    turn: Option<&str>,
    now: &str,
) -> Result<Receipt> {
    if receipt.revision == i64::MAX {
        return Err(invalid("native receipt revision exhausted"));
    }
    if !valid_text(now, 128) {
        return Err(invalid("invalid native receipt clock"));
    }
    affected(tx.execute("UPDATE native_execution_receipts SET phase=?1,prompt_id=?2,turn_id=?3,revision=revision+1,updated_at=?4 WHERE request_id=?5 AND revision=?6",params![phase.key(),prompt,turn,now,receipt.request_id,receipt.revision])?,1)?;
    get(tx, &receipt.request_id)?.ok_or_else(|| invalid("native receipt update missing"))
}
pub fn accept_content(
    tx: &Transaction<'_>,
    runtime: &str,
    request: &str,
    revision: i64,
    prompt: &str,
    body: &[u8],
    now: &str,
) -> Result<Receipt> {
    let receipt = owned(tx, runtime, request, revision)?;
    if active(tx, runtime)?.0 || receipt.phase != Phase::Prepared || !uuid(prompt) {
        return Err(invalid("native content admission unavailable"));
    }
    let (original, witness): (Vec<u8>, Vec<u8>) = tx.query_row(
        "SELECT body,witness FROM native_execution_inputs WHERE request_id=?1",
        [request],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if original != body || witness != digest(&SHA256, body).as_ref() {
        return Err(invalid("native effective content differs"));
    }
    update(
        tx,
        &receipt,
        Phase::ContentAccepted,
        Some(prompt),
        None,
        now,
    )
}
/// Live managed-hook authority, minted only by a fresh Core-owned registration.
/// No Serde/row constructor: restoring a DB cannot recreate dispatch authority.
#[derive(Debug)]
pub struct ManagedRuntime {
    runtime: Runtime,
}
impl ManagedRuntime {
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }
}
pub fn register_managed(tx: &Transaction<'_>, runtime: &Runtime) -> Result<ManagedRuntime> {
    register(tx, runtime)?;
    Ok(ManagedRuntime {
        runtime: runtime.clone(),
    })
}
fn managed_current(tx: &Transaction<'_>, seal: &ManagedRuntime) -> Result<()> {
    if current_runtime(tx, &seal.runtime.session)?.as_ref() != Some(&seal.runtime)
        || active(tx, &seal.runtime.id)?.0
    {
        return Err(invalid("managed live authority unavailable"));
    }
    Ok(())
}
/// Final hook authorization under the same Core terminal lease gate as raw claim.
pub fn verify_managed_authority(tx: &Transaction<'_>, seal: &ManagedRuntime) -> Result<()> {
    managed_current(tx, seal)
}
/// A real prompt UUID may bind only one request in this managed execution.
pub fn managed_accept_content(
    tx: &Transaction<'_>,
    seal: &ManagedRuntime,
    request: &str,
    revision: i64,
    prompt: &str,
    body: &[u8],
    now: &str,
) -> Result<Receipt> {
    managed_current(tx, seal)?;
    let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_execution_receipts WHERE runtime_id=?1 AND prompt_id=?2)",params![seal.runtime.id,prompt],|r|r.get(0))?;
    if duplicate {
        return Err(invalid("managed prompt UUID already bound"));
    }
    accept_content(tx, &seal.runtime.id, request, revision, prompt, body, now)
}
/// Response waiting is deliberately not a native turn.complete. The only writer
/// of this Completed/NULL shape owns a fresh managed seal and real prompt UUID.
/// Native turn_* methods continue requiring their separate genuine turn ID.
pub fn managed_response_waiting(
    tx: &Transaction<'_>,
    seal: &ManagedRuntime,
    request: &str,
    revision: i64,
    prompt: &str,
    now: &str,
) -> Result<Receipt> {
    managed_current(tx, seal)?;
    let receipt = owned(tx, &seal.runtime.id, request, revision)?;
    if receipt.phase != Phase::ContentAccepted
        || receipt.prompt_id.as_deref() != Some(prompt)
        || receipt.turn_id.is_some()
        || !uuid(prompt)
    {
        return Err(invalid("managed response waiting unbound"));
    }
    let completed = update(tx, &receipt, Phase::Completed, Some(prompt), None, now)?;
    if tx.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? >= 19
        && crate::notifications::settings(tx)?.enabled
    {
        let crew: String = tx.query_row(
            "SELECT crew_id FROM sessions WHERE id=?1",
            [&seal.runtime.session],
            |r| r.get(0),
        )?;
        crate::notifications::enqueue(
            tx,
            &crew,
            crewd_proto::notifications::NotificationKind::ReplyReady,
            crewd_proto::notifications::NotificationSource::ManagedResponse {
                session_id: seal.runtime.session.clone(),
                runtime_id: seal.runtime.id.clone(),
                request_id: request.into(),
                prompt_id: prompt.into(),
                receipt_revision: completed.revision.to_string(),
            },
            Some((&seal.runtime.session, &seal.runtime.generation)),
            crate::now_ms(),
        )?;
    }
    Ok(completed)
}
/// A later genuine same-prompt activity retracts only this managed waiting
/// observation. It can never rewrite a native turn completion.
pub fn managed_response_resumed(
    tx: &Transaction<'_>,
    seal: &ManagedRuntime,
    prompt: &str,
    now: &str,
) -> Result<()> {
    managed_current(tx, seal)?;
    if !uuid(prompt) {
        return Err(invalid("managed prompt invalid"));
    }
    let request:Option<String>=tx.query_row("SELECT request_id FROM native_execution_receipts WHERE runtime_id=?1 AND prompt_id=?2 AND turn_id IS NULL AND phase='completed'",params![seal.runtime.id,prompt],|r|r.get(0)).optional()?;
    if let Some(request) = request {
        let receipt = get(tx, &request)?.ok_or_else(|| invalid("managed waiting vanished"))?;
        update(tx, &receipt, Phase::Unknown, Some(prompt), None, now)?;
        if tx.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? >= 19 {
            crate::notifications::suppress_session(tx, &seal.runtime.session, "response_resumed")?;
        }
    }
    Ok(())
}
pub fn turn_started(
    tx: &Transaction<'_>,
    runtime: &str,
    request: &str,
    revision: i64,
    prompt: &str,
    turn: &str,
    now: &str,
) -> Result<Receipt> {
    let receipt = owned(tx, runtime, request, revision)?;
    if receipt.prompt_id.as_deref() != Some(prompt)
        || !valid_text(turn, 256)
        || receipt.turn_id.is_some()
    {
        return Err(invalid("native turn start unbound"));
    }
    let phase = match receipt.phase {
        Phase::ContentAccepted if !active(tx, runtime)?.0 => Phase::Running,
        // A genuine late start supplies the missing completion identity. It
        // never undoes cancellation, uncertainty or manual ownership.
        Phase::CancelRequested | Phase::RawRevoked | Phase::Unknown => receipt.phase,
        _ => return Err(invalid("native turn start unbound")),
    };
    update(tx, &receipt, phase, Some(prompt), Some(turn), now)
}
pub fn request_cancel(
    tx: &Transaction<'_>,
    runtime: &str,
    request: &str,
    revision: i64,
    now: &str,
) -> Result<Receipt> {
    let receipt = owned(tx, runtime, request, revision)?;
    if active(tx, runtime)?.0
        || !matches!(
            receipt.phase,
            Phase::Prepared | Phase::ContentAccepted | Phase::Running
        )
    {
        return Err(invalid("native cancellation unavailable"));
    }
    update(
        tx,
        &receipt,
        Phase::CancelRequested,
        receipt.prompt_id.as_deref(),
        receipt.turn_id.as_deref(),
        now,
    )
}
/// Commit ACK is required before the caller forwards any manual PTY bytes.
pub fn revoke_before_raw(
    tx: &Transaction<'_>,
    runtime: &str,
    revision: i64,
    now: &str,
) -> Result<()> {
    let (_, actual) = active(tx, runtime)?;
    if actual != revision || actual == i64::MAX || !valid_text(now, 128) {
        return Err(invalid("native manual CAS conflict"));
    }
    let count = pending(tx, runtime)?;
    affected(tx.execute("UPDATE native_execution_runtimes SET manual=1,revision=revision+1 WHERE runtime_id=?1 AND revision=?2",params![runtime,revision])?,1)?;
    affected(tx.execute("UPDATE native_execution_receipts SET phase='raw_revoked',revision=revision+1,updated_at=?2 WHERE runtime_id=?1 AND phase NOT IN('completed','aborted','failed')",params![runtime,now])?,count)?;
    Ok(())
}
/// Only an authenticated matching actual turn.complete may call this. It does
/// not clear runtime.manual, so an old automatic completion cannot release raw.
#[derive(Debug, Clone, Copy)]
pub struct Completion<'a> {
    pub prompt_id: &'a str,
    pub turn_id: &'a str,
    pub outcome: Phase,
}
pub fn turn_completed(
    tx: &Transaction<'_>,
    runtime: &str,
    request: &str,
    revision: i64,
    completion: Completion<'_>,
    now: &str,
) -> Result<Receipt> {
    let Completion {
        prompt_id: prompt,
        turn_id: turn,
        outcome,
    } = completion;
    let receipt = owned(tx, runtime, request, revision)?;
    if !outcome.settled()
        || receipt.phase.settled()
        || receipt.prompt_id.as_deref() != Some(prompt)
        || receipt.turn_id.as_deref() != Some(turn)
    {
        return Err(invalid("native completion unbound"));
    }
    update(tx, &receipt, outcome, Some(prompt), Some(turn), now)
}
/// Owning Core must first verify its actual registered kernel birth is gone.
/// This closes the old runtime, never regrants it or claims model success.
pub fn retire_verified_native(tx: &Transaction<'_>, runtime: &str, now: &str) -> Result<()> {
    let (_, revision) = active(tx, runtime)?;
    let overflow:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_execution_receipts WHERE runtime_id=?1 AND phase NOT IN('completed','aborted','failed') AND revision>=9223372036854775807)",[runtime],|r|r.get(0))?;
    if overflow || revision == i64::MAX {
        return Err(invalid("native retirement revision exhausted"));
    }
    if !valid_text(now, 128) {
        return Err(invalid("invalid retirement clock"));
    }
    let count = pending(tx, runtime)?;
    affected(tx.execute(
        "UPDATE native_execution_runtimes SET revoked=1,revision=revision+1 WHERE runtime_id=?1",
        [runtime],
    )?,1)?;
    affected(tx.execute("UPDATE native_execution_receipts SET phase='failed',revision=revision+1,updated_at=?2 WHERE runtime_id=?1 AND phase NOT IN('completed','aborted','failed')",params![runtime,now])?,count)?;
    Ok(())
}
/// A disconnected private bridge only invalidates its owning runtime. No
/// other provider/crew is affected and no observed process is forgotten.
pub fn invalidate_runtime_unknown(tx: &Transaction<'_>, runtime: &str) -> Result<()> {
    let count = pending(tx, runtime)?;
    let (_, revision) = active(tx, runtime)?;
    if revision == i64::MAX {
        return Err(invalid("native runtime revision exhausted"));
    }
    affected(tx.execute("UPDATE native_execution_runtimes SET revoked=1,manual=1,revision=revision+1 WHERE runtime_id=?1 AND revoked=0",[runtime])?,1)?;
    affected(tx.execute("UPDATE native_execution_receipts SET phase='unknown',revision=revision+1 WHERE runtime_id=?1 AND phase NOT IN('completed','aborted','failed')",[runtime])?,count)?;
    Ok(())
}
/// Run on Core restart and on the staged restore/import copy before installing
/// it. Old schemas are a no-op; stored native bindings are never fresh authority.
pub fn invalidate_unknown(tx: &rusqlite::Connection) -> Result<usize> {
    if tx.is_autocommit() {
        return Err(invalid("native invalidation requires owning transaction"));
    }
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_execution_runtimes')",[],|r|r.get(0))?;
    if !exists {
        return Ok(0);
    }
    let overflow:bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM native_execution_runtimes WHERE revoked=0 AND revision>=9223372036854775807) OR EXISTS(SELECT 1 FROM native_execution_receipts WHERE phase NOT IN('completed','aborted','failed','unknown') AND revision>=9223372036854775807)",[],|r|r.get(0))?;
    if overflow {
        return Err(invalid("native invalidation revision exhausted"));
    }
    let runtimes = usize::try_from(tx.query_row(
        "SELECT COUNT(*) FROM native_execution_runtimes WHERE revoked=0",
        [],
        |r| r.get::<_, i64>(0),
    )?)
    .map_err(|_| invalid("native runtime count overflow"))?;
    let receipts=usize::try_from(tx.query_row("SELECT COUNT(*) FROM native_execution_receipts WHERE phase NOT IN('completed','aborted','failed','unknown')",[],|r|r.get::<_,i64>(0))?).map_err(|_|invalid("native receipt count overflow"))?;
    affected(tx.execute("UPDATE native_execution_runtimes SET revoked=1,manual=1,revision=revision+1 WHERE revoked=0",[])?,runtimes)?;
    affected(tx.execute("UPDATE native_execution_receipts SET phase='unknown',revision=revision+1 WHERE phase NOT IN('completed','aborted','failed','unknown')",[])?,receipts)?;
    Ok(receipts)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn setup() -> (rusqlite::Connection, Runtime) {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE sessions(id TEXT PRIMARY KEY); INSERT INTO sessions VALUES('session');").unwrap();
        c.execute_batch(V12_DDL).unwrap();
        let runtime = Runtime {
            id: id(),
            session: "session".into(),
            generation: id(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "boot".into(),
        };
        let tx = c.unchecked_transaction().unwrap();
        register(&tx, &runtime).unwrap();
        tx.commit().unwrap();
        (c, runtime)
    }
    #[test]
    fn managed_waiting_has_no_native_turn_and_unknown_restore_never_recreates_seal() {
        let (c, old) = setup();
        let tx = c.unchecked_transaction().unwrap();
        assert!(
            register_managed(&tx, &old).is_err(),
            "a row cannot be converted into a fresh managed seal"
        );
        invalidate_runtime_unknown(&tx, &old.id).unwrap();
        let mut runtime = old.clone();
        runtime.id = id();
        runtime.generation = id();
        let seal = register_managed(&tx, &runtime).unwrap();
        let request = id();
        let prompt = id();
        let body = b"<managed-id> exact original task ";
        let (prepared, reused) = prepare(&tx, &runtime.id, &request, body, "now").unwrap();
        assert!(!reused);
        assert!(
            managed_response_waiting(&tx, &seal, &request, prepared.revision, &prompt, "now")
                .is_err()
        );
        let accepted =
            managed_accept_content(&tx, &seal, &request, 0, &prompt, body, "now").unwrap();
        assert!(
            managed_response_waiting(&tx, &seal, &request, accepted.revision, &id(), "now")
                .is_err()
        );
        let waiting =
            managed_response_waiting(&tx, &seal, &request, accepted.revision, &prompt, "now")
                .unwrap();
        assert_eq!(waiting.phase, Phase::Completed);
        assert_eq!(waiting.turn_id, None);
        assert!(
            turn_completed(
                &tx,
                &runtime.id,
                &request,
                waiting.revision,
                Completion {
                    prompt_id: &prompt,
                    turn_id: "invented",
                    outcome: Phase::Completed
                },
                "now"
            )
            .is_err()
        );
        let (same, reused) = prepare(&tx, &runtime.id, &request, body, "later").unwrap();
        assert!(reused);
        assert_eq!(same, waiting);
        let next = id();
        prepare(&tx, &runtime.id, &next, body, "later").unwrap();
        assert!(
            managed_accept_content(&tx, &seal, &next, 0, &prompt, body, "later").is_err(),
            "real prompt UUID cannot be reused by another managed request"
        );
        invalidate_unknown(&tx).unwrap();
        assert_eq!(get(&tx, &next).unwrap().unwrap().phase, Phase::Unknown);
        assert_eq!(
            get(&tx, &request).unwrap().unwrap(),
            waiting,
            "waiting observation remains historical, never native turn completion"
        );
        assert!(current_runtime(&tx, &runtime.session).unwrap().is_none());
        assert!(managed_response_waiting(&tx, &seal, &next, 1, &prompt, "later").is_err());
        let (saved, reused) = prepare(&tx, &runtime.id, &next, body, "later").unwrap();
        assert!(reused);
        assert_eq!(
            saved.phase,
            Phase::Unknown,
            "same UUID is only original-state lookup, never dispatch authority"
        );
        assert!(prepare(&tx, &runtime.id, &id(), body, "later").is_err());
        tx.commit().unwrap();
    }

    #[test]
    fn managed_late_activity_retracts_waiting_without_changing_native_completion() {
        let (c, old) = setup();
        let tx = c.unchecked_transaction().unwrap();
        invalidate_runtime_unknown(&tx, &old.id).unwrap();
        let mut runtime = old.clone();
        runtime.id = id();
        runtime.generation = id();
        let seal = register_managed(&tx, &runtime).unwrap();
        let request = id();
        let prompt = id();
        prepare(&tx, &runtime.id, &request, b"task", "now").unwrap();
        let accepted =
            managed_accept_content(&tx, &seal, &request, 0, &prompt, b"task", "now").unwrap();
        managed_response_waiting(&tx, &seal, &request, accepted.revision, &prompt, "now").unwrap();
        managed_response_resumed(&tx, &seal, &prompt, "later").unwrap();
        assert_eq!(get(&tx, &request).unwrap().unwrap().phase, Phase::Unknown);
        assert!(
            managed_response_waiting(&tx, &seal, &request, accepted.revision, &prompt, "later")
                .is_err()
        );
    }

    #[test]
    fn managed_waiting_cannot_cross_runtime_or_raw_floor() {
        let (c, old) = setup();
        let tx = c.unchecked_transaction().unwrap();
        invalidate_runtime_unknown(&tx, &old.id).unwrap();
        let mut runtime = old.clone();
        runtime.id = id();
        runtime.generation = id();
        let seal = register_managed(&tx, &runtime).unwrap();
        let request = id();
        let prompt = id();
        prepare(&tx, &runtime.id, &request, b"task", "now").unwrap();
        let accepted =
            managed_accept_content(&tx, &seal, &request, 0, &prompt, b"task", "now").unwrap();
        revoke_before_raw(&tx, &runtime.id, 0, "now").unwrap();
        assert!(
            managed_response_waiting(&tx, &seal, &request, accepted.revision + 1, &prompt, "now")
                .is_err()
        );
        assert_eq!(
            get(&tx, &request).unwrap().unwrap().phase,
            Phase::RawRevoked
        );
        tx.rollback().unwrap();
    }

    #[test]
    fn exact_immutable_content_and_cas_are_transactional_and_metadata_has_no_body() {
        let (c, r) = setup();
        let request = id();
        let prompt = id();
        let body = b" a \n\xe4\xb8\x96\xe7\x95\x8c\n";
        let tx = c.unchecked_transaction().unwrap();
        let (first, reused) = prepare(&tx, &r.id, &request, body, "now").unwrap();
        assert!(!reused);
        tx.commit().unwrap();
        assert!(!format!("{first:?}").contains("世界"));
        assert_eq!(
            list_metadata(&c, &r.id, None, 64).unwrap(),
            vec![first.clone()]
        );
        let input = original_input(&c, &r.id, &request).unwrap().unwrap();
        assert_eq!(input.bytes(), body);
        assert!(!format!("{input:?}").contains("世界"));
        assert!(original_input(&c, &id(), &request).unwrap().is_none());
        let tx = c.unchecked_transaction().unwrap();
        assert!(accept_content(&tx, &r.id, &request, 0, &prompt, b"a \n", "now").is_err());
        assert!(accept_content(&tx, &r.id, &request, 1, &prompt, body, "now").is_err());
        tx.rollback().unwrap();
        assert_eq!(get(&c, &request).unwrap(), Some(first));
        let tx = c.unchecked_transaction().unwrap();
        let accepted = accept_content(&tx, &r.id, &request, 0, &prompt, body, "now").unwrap();
        let running = turn_started(
            &tx,
            &r.id,
            &request,
            accepted.revision,
            &prompt,
            "turn",
            "now",
        )
        .unwrap();
        let complete = turn_completed(
            &tx,
            &r.id,
            &request,
            running.revision,
            Completion {
                prompt_id: &prompt,
                turn_id: "turn",
                outcome: Phase::Completed,
            },
            "now",
        )
        .unwrap();
        tx.commit().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        let (retry, reused) = prepare(&tx, &r.id, &request, body, "later").unwrap();
        assert!(reused);
        assert_eq!(retry, complete);
        assert!(prepare(&tx, &r.id, &request, b"changed", "later").is_err());
        assert!(
            tx.execute("UPDATE native_execution_inputs SET body=x'61'", [])
                .is_err()
        );
        tx.rollback().unwrap();
    }
    #[test]
    fn raw_revocation_survives_old_complete_and_restore_never_regrants() {
        let (c, r) = setup();
        let request = id();
        let prompt = id();
        let tx = c.unchecked_transaction().unwrap();
        prepare(&tx, &r.id, &request, b"body", "now").unwrap();
        let a = accept_content(&tx, &r.id, &request, 0, &prompt, b"body", "now").unwrap();
        let b = turn_started(&tx, &r.id, &request, a.revision, &prompt, "turn", "now").unwrap();
        let cancel = request_cancel(&tx, &r.id, &request, b.revision, "now").unwrap();
        assert_eq!(cancel.phase, Phase::CancelRequested);
        assert!(prepare(&tx, &r.id, &id(), b"next", "now").is_err());
        revoke_before_raw(&tx, &r.id, 0, "now").unwrap();
        let revoked = get(&tx, &request).unwrap().unwrap();
        turn_completed(
            &tx,
            &r.id,
            &request,
            revoked.revision,
            Completion {
                prompt_id: &prompt,
                turn_id: "turn",
                outcome: Phase::Aborted,
            },
            "now",
        )
        .unwrap();
        assert!(prepare(&tx, &r.id, &id(), b"next", "now").is_err());
        tx.commit().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        invalidate_unknown(&tx).unwrap();
        tx.commit().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(prepare(&tx, &r.id, &id(), b"revive", "now").is_err());
        assert!(accept_content(&tx, &r.id, &request, 4, &prompt, b"body", "now").is_err());
        tx.rollback().unwrap();
    }
    #[test]
    fn actual_sql_abort_rolls_back_input_identity_and_raw_permission() {
        let (c, r) = setup();
        c.execute_batch("CREATE TRIGGER refuse_body BEFORE INSERT ON native_execution_inputs BEGIN SELECT RAISE(ABORT,'real input fault'); END;").unwrap();
        let request = id();
        let tx = c.unchecked_transaction().unwrap();
        assert!(prepare(&tx, &r.id, &request, b"body", "now").is_err());
        tx.rollback().unwrap();
        assert!(get(&c, &request).unwrap().is_none());
        c.execute_batch("DROP TRIGGER refuse_body; CREATE TRIGGER refuse_raw BEFORE UPDATE ON native_execution_runtimes WHEN NEW.manual=1 BEGIN SELECT RAISE(ABORT,'real raw fault'); END;").unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(revoke_before_raw(&tx, &r.id, 0, "now").is_err());
        tx.rollback().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(!active(&tx, &r.id).unwrap().0);
        tx.rollback().unwrap();
    }
    #[tokio::test]
    async fn core_fifo_requires_commit_ack_and_unknown_reply_never_replays_input() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let store = crate::Store::open_in_memory().unwrap();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('receipt-crew','fixture','now','now'); INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('receipt-session','receipt-crew','worker','claude','now');").unwrap();
        // The actual Store installs formal12 before exposing its FIFO actor.
        let runtime = Runtime {
            id: id(),
            session: "receipt-session".into(),
            generation: id(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "fixture-birth".into(),
        };
        let tx = store.conn().unchecked_transaction().unwrap();
        register(&tx, &runtime).unwrap();
        tx.commit().unwrap();
        let db = crate::Db::spawn(store).unwrap();
        let request = id();
        let expected_request = request.clone();
        let runtime_id = runtime.id.clone();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, hold) = std::sync::mpsc::channel();
        let emitted = Arc::new(AtomicBool::new(false));
        let sent = emitted.clone();
        let owned_db = db.clone();
        let task = tokio::spawn(async move {
            let result = owned_db
                .call_result(move |store| -> Result<Receipt> {
                    let tx = store.conn().unchecked_transaction()?;
                    let (receipt, reused) =
                        prepare(&tx, &runtime_id, &request, b"exact immutable body", "now")?;
                    assert!(!reused);
                    entered.send(()).unwrap();
                    hold.recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    tx.commit()?;
                    Ok(receipt)
                })
                .await;
            if result.as_ref().is_ok_and(|inner| inner.is_ok()) {
                sent.store(true, Ordering::SeqCst);
            }
            result
        });
        waiting.await.unwrap();
        assert!(
            !emitted.load(Ordering::SeqCst),
            "no native effect while commit is held"
        );
        // Losing an already admitted caller is an unknown response, not a
        // rollback promise or permission to resend native input.
        task.abort();
        let _ = task.await;
        release.send(()).unwrap();
        let runtime_id = runtime.id.clone();
        let request = expected_request.clone();
        let confirmation = db
            .call_result(move |store| -> Result<(Receipt, bool)> {
                let tx = store.conn().unchecked_transaction()?;
                let value = prepare(&tx, &runtime_id, &request, b"exact immutable body", "later")?;
                tx.commit()?;
                Ok(value)
            })
            .await
            .unwrap()
            .unwrap();
        assert!(confirmation.1);
        assert_eq!(confirmation.0.phase, Phase::Prepared);
        assert!(
            !emitted.load(Ordering::SeqCst),
            "unknown response did not emit or replay"
        );
        let runtime_id = runtime.id.clone();
        db.call_result(move|store|->Result<()> {
            assert_eq!(list_metadata(store.conn(),&runtime_id,None,64)?.len(),1);
            store.conn().execute_batch("CREATE TRIGGER deny_raw_phase BEFORE UPDATE ON native_execution_receipts BEGIN SELECT RAISE(IGNORE); END;")?;
            Ok(())
        }).await.unwrap().unwrap();
        let runtime_id = runtime.id.clone();
        let refused = db
            .call_result(move |store| -> Result<()> {
                let tx = store.conn().unchecked_transaction()?;
                revoke_before_raw(&tx, &runtime_id, 0, "now")?;
                tx.commit()?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            refused.is_err(),
            "failed durable raw phase cannot return an ACK"
        );
        let runtime_id = runtime.id.clone();
        let state = db
            .call_result(move |store| -> Result<(bool, i64)> {
                let tx = store.conn().unchecked_transaction()?;
                active(&tx, &runtime_id)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state,
            (false, 0),
            "the same Core FIFO rolled back the earlier raw write"
        );
        db.shutdown();
    }
    #[test]
    fn actual_sql_ignore_cannot_issue_content_or_raw_commit_permission() {
        let (c, r) = setup();
        let request = id();
        c.execute_batch("CREATE TRIGGER ignore_input BEFORE INSERT ON native_execution_inputs BEGIN SELECT RAISE(IGNORE); END;").unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(prepare(&tx, &r.id, &request, b"immutable body", "now").is_err());
        tx.rollback().unwrap();
        assert!(get(&c, &request).unwrap().is_none());
        c.execute_batch("DROP TRIGGER ignore_input;").unwrap();
        let tx = c.unchecked_transaction().unwrap();
        let original = prepare(&tx, &r.id, &request, b"immutable body", "now")
            .unwrap()
            .0;
        tx.commit().unwrap();
        c.execute_batch("CREATE TRIGGER ignore_phase BEFORE UPDATE ON native_execution_receipts BEGIN SELECT RAISE(IGNORE); END;").unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(
            accept_content(
                &tx,
                &r.id,
                &request,
                original.revision,
                &id(),
                b"immutable body",
                "now"
            )
            .is_err()
        );
        tx.rollback().unwrap();
        assert_eq!(get(&c, &request).unwrap(), Some(original.clone()));
        let tx = c.unchecked_transaction().unwrap();
        assert!(revoke_before_raw(&tx, &r.id, 0, "now").is_err());
        tx.rollback().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert_eq!(active(&tx, &r.id).unwrap(), (false, 0));
        tx.rollback().unwrap();
        assert_eq!(get(&c, &request).unwrap(), Some(original));
        let tx = c.unchecked_transaction().unwrap();
        assert!(invalidate_unknown(&tx).is_err());
        tx.rollback().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert_eq!(active(&tx, &r.id).unwrap(), (false, 0));
        tx.rollback().unwrap();
    }
    #[test]
    fn authenticated_late_turn_start_records_identity_without_regrant_after_cancel_or_raw() {
        for raw in [false, true] {
            let (c, r) = setup();
            let request = id();
            let prompt = id();
            let tx = c.unchecked_transaction().unwrap();
            prepare(&tx, &r.id, &request, b"body", "now").unwrap();
            let accepted =
                accept_content(&tx, &r.id, &request, 0, &prompt, b"body", "now").unwrap();
            if raw {
                revoke_before_raw(&tx, &r.id, 0, "now").unwrap();
            } else {
                request_cancel(&tx, &r.id, &request, accepted.revision, "now").unwrap();
            }
            let pending = get(&tx, &request).unwrap().unwrap();
            assert!(
                turn_started(
                    &tx,
                    &r.id,
                    &request,
                    pending.revision,
                    &id(),
                    "wrong-prompt",
                    "now"
                )
                .is_err()
            );
            let started = turn_started(
                &tx,
                &r.id,
                &request,
                pending.revision,
                &prompt,
                "actual-late-turn",
                "now",
            )
            .unwrap();
            assert_eq!(
                started.phase, pending.phase,
                "late fact must not grant Running again"
            );
            assert!(prepare(&tx, &r.id, &id(), b"next", "now").is_err());
            let settled = turn_completed(
                &tx,
                &r.id,
                &request,
                started.revision,
                Completion {
                    prompt_id: &prompt,
                    turn_id: "actual-late-turn",
                    outcome: Phase::Aborted,
                },
                "now",
            )
            .unwrap();
            assert_eq!(settled.phase, Phase::Aborted);
            if raw {
                assert!(prepare(&tx, &r.id, &id(), b"next", "now").is_err());
            }
            tx.commit().unwrap();
        }
    }
    #[test]
    fn restored_pending_receipt_becomes_unknown_and_old_schema_invalidation_is_noop() {
        let (c, r) = setup();
        let request = id();
        let tx = c.unchecked_transaction().unwrap();
        prepare(&tx, &r.id, &request, b"unconfirmed", "now").unwrap();
        tx.commit().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert_eq!(invalidate_unknown(&tx).unwrap(), 1);
        tx.commit().unwrap();
        assert_eq!(get(&c, &request).unwrap().unwrap().phase, Phase::Unknown);
        let old = rusqlite::Connection::open_in_memory().unwrap();
        let tx = old.unchecked_transaction().unwrap();
        assert_eq!(invalidate_unknown(&tx).unwrap(), 0);
    }
    #[test]
    fn actual_restore_staging_revokes_native_before_install_and_retains_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.db");
        let dest = tmp.path().join("restored");
        std::fs::create_dir(&dest).unwrap();
        let store = crate::Store::open(&source).unwrap();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('c','fixture','now','now'); INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('s','c','worker','claude','now');").unwrap();
        let runtime = Runtime {
            id: id(),
            session: "s".into(),
            generation: id(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "synthetic-kernel-fixture".into(),
        };
        let request = id();
        let tx = store.conn().unchecked_transaction().unwrap();
        register(&tx, &runtime).unwrap();
        prepare(&tx, &runtime.id, &request, b"exact original\n", "now").unwrap();
        revoke_before_raw(&tx, &runtime.id, 0, "now").unwrap();
        tx.commit().unwrap();
        crate::backup::restore(&dest, &source).unwrap();
        // Inspect installed file BEFORE any daemon startup could invalidate it.
        let installed = rusqlite::Connection::open(dest.join(crate::DB_FILE)).unwrap();
        let receipt = get(&installed, &request).unwrap().unwrap();
        assert_eq!(receipt.phase, Phase::Unknown);
        assert_eq!(
            get(store.conn(), &request).unwrap().unwrap().phase,
            Phase::RawRevoked
        );
        assert_eq!(
            installed
                .query_row(
                    "SELECT revoked,manual FROM native_execution_runtimes WHERE runtime_id=?1",
                    [&runtime.id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                )
                .unwrap(),
            (1, 1)
        );
        let tx = installed.unchecked_transaction().unwrap();
        assert!(
            accept_content(
                &tx,
                &runtime.id,
                &request,
                receipt.revision,
                &id(),
                b"exact original\n",
                "later"
            )
            .is_err()
        );
        assert!(revoke_before_raw(&tx, &runtime.id, 1, "later").is_err());
        let (confirmed, already) =
            prepare(&tx, &runtime.id, &request, b"exact original\n", "later").unwrap();
        assert!(already);
        assert_eq!(confirmed.phase, Phase::Unknown);
        assert_eq!(invalidate_unknown(&tx).unwrap(), 0);
        tx.commit().unwrap();
    }
}
