//! The sole intent/API-outcome authority for managed native operations.
//! Native turn identity and work completion are deliberately not inferred here.
use crate::{Result, StoreError, native_execution::Runtime};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_BODY: usize = 1_048_576;
pub const MAX_RESULT: usize = 1_048_576;
pub const V16_DDL: &str = r#"
CREATE TABLE native_managed_operations(
 operation_id TEXT PRIMARY KEY,
 runtime_id TEXT NOT NULL REFERENCES native_execution_runtimes(runtime_id) ON DELETE CASCADE,
 principal TEXT NOT NULL CHECK(length(principal) BETWEEN 1 AND 512),
 explicit_human INTEGER NOT NULL CHECK(explicit_human IN (0,1)),
 kind TEXT NOT NULL CHECK(kind IN ('prompt','cancel','permission')),
 body BLOB NOT NULL CHECK(length(body) BETWEEN 1 AND 1048576),
 body_sha256 BLOB NOT NULL CHECK(length(body_sha256)=32),
 phase TEXT NOT NULL CHECK(phase IN ('prepared','dispatched','returned','unknown')),
 result_json TEXT CHECK(result_json IS NULL OR (json_valid(result_json) AND length(CAST(result_json AS BLOB))<=1048576)),
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 CHECK(phase!='returned' OR result_json IS NOT NULL)
);
CREATE INDEX native_managed_runtime ON native_managed_operations(runtime_id,phase);
CREATE TRIGGER native_managed_identity_immutable BEFORE UPDATE OF operation_id,runtime_id,principal,explicit_human,kind,body,body_sha256,created_at ON native_managed_operations BEGIN
 SELECT RAISE(ABORT,'managed operation identity is immutable');
END;
"#;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Dispatched,
    Returned,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Prompt,
    Cancel,
    Permission,
}
impl Kind {
    fn key(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Cancel => "cancel",
            Self::Permission => "permission",
        }
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub operation_id: String,
    pub runtime_id: String,
    pub principal: String,
    pub explicit_human: bool,
    pub kind: Kind,
    pub phase: Phase,
    pub result: Option<Value>,
}
impl std::fmt::Debug for Receipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedReceipt")
            .field("operation_id", &self.operation_id)
            .field("phase", &self.phase)
            .field("result", &"<private>")
            .finish()
    }
}
fn invalid(s: &'static str) -> StoreError {
    StoreError::Invalid(s.into())
}
fn uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|v| !v.is_nil() && v.to_string() == s)
}
fn live(c: &rusqlite::Connection, runtime: &Runtime) -> Result<()> {
    let actual = crate::native_execution::current_runtime(c, &runtime.session)?;
    if actual.as_ref() != Some(runtime) {
        return Err(invalid("managed native runtime changed"));
    }
    let manual: bool = c.query_row(
        "SELECT manual FROM native_execution_runtimes WHERE runtime_id=?1",
        [&runtime.id],
        |r| r.get(0),
    )?;
    if manual {
        return Err(invalid("managed runtime handed to manual control"));
    }
    Ok(())
}
pub fn get(c: &rusqlite::Connection, operation: &str) -> Result<Option<Receipt>> {
    let row:Option<(String,String,bool,String,String,Option<String>)>=c.query_row(
        "SELECT runtime_id,principal,explicit_human,kind,phase,result_json FROM native_managed_operations WHERE operation_id=?1",[operation],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))
    ).optional()?;
    row.map(
        |(runtime_id, principal, explicit_human, kind, phase, result)| {
            Ok(Receipt {
                operation_id: operation.into(),
                runtime_id,
                principal,
                explicit_human,
                kind: serde_json::from_value(Value::String(kind))?,
                phase: serde_json::from_value(Value::String(phase))?,
                result: result.map(|raw| serde_json::from_str(&raw)).transpose()?,
            })
        },
    )
    .transpose()
}
/// The caller rechecks current ACL and ephemeral run admission in this same
/// FIFO transaction. A duplicate is attribution only, including after restart.
pub fn prepare(
    tx: &Transaction<'_>,
    runtime: &Runtime,
    principal: &str,
    explicit_human: bool,
    kind: Kind,
    operation: &str,
    body: &[u8],
    now: &str,
) -> Result<(Receipt, bool)> {
    if !uuid(operation)
        || body.is_empty()
        || body.len() > MAX_BODY
        || principal.is_empty()
        || principal.len() > 512
        || now.is_empty()
    {
        return Err(invalid("managed operation bounds invalid"));
    }
    std::str::from_utf8(body).map_err(|_| invalid("managed body is not UTF8"))?;
    let sha = ring::digest::digest(&ring::digest::SHA256, body);
    if let Some(old) = get(tx, operation)? {
        let (old_body, old_sha): (Vec<u8>, Vec<u8>) = tx.query_row(
            "SELECT body,body_sha256 FROM native_managed_operations WHERE operation_id=?1",
            [operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if old.runtime_id != runtime.id
            || old.principal != principal
            || old.explicit_human != explicit_human
            || old.kind != kind
            || old_body != body
            || old_sha != sha.as_ref()
        {
            return Err(invalid("managed operation immutable identity conflict"));
        }
        return Ok((old, true));
    }
    live(tx, runtime)?;
    // An explicit distinct Human intention may follow an uncertain API call.
    // Automatic work must have actual readiness evidence at the owning runner.
    if !explicit_human {
        let uncertain:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_managed_operations WHERE runtime_id=?1 AND kind='prompt')",[&runtime.id],|r|r.get(0))?;
        if uncertain {
            return Err(invalid("managed automatic readiness remains unknown"));
        }
    }
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM native_managed_operations WHERE runtime_id=?1",
        [&runtime.id],
        |r| r.get(0),
    )?;
    if count >= 4096 {
        return Err(invalid("managed operation capacity exhausted"));
    }
    tx.execute(
        "INSERT INTO native_managed_operations VALUES(?1,?2,?3,?4,?5,?6,?7,'prepared',NULL,?8,?8)",
        params![
            operation,
            runtime.id,
            principal,
            explicit_human,
            kind.key(),
            body,
            sha.as_ref(),
            now
        ],
    )?;
    Ok((
        get(tx, operation)?.ok_or_else(|| invalid("managed intent missing"))?,
        false,
    ))
}
/// Only a newly changed Prepared->Dispatched commit may release one API call.
/// A cancelled waiter/duplicate/restart must not recover a dispatch permit.
pub fn release(
    tx: &Transaction<'_>,
    runtime: &Runtime,
    operation: &str,
    now: &str,
) -> Result<bool> {
    live(tx, runtime)?;
    let old = get(tx, operation)?
        .filter(|r| r.runtime_id == runtime.id)
        .ok_or_else(|| invalid("managed intent unavailable"))?;
    if old.phase != Phase::Prepared {
        return Ok(false);
    }
    if tx.execute("UPDATE native_managed_operations SET phase='dispatched',updated_at=?2 WHERE operation_id=?1 AND phase='prepared'",params![operation,now])?!=1 {
        return Err(invalid("managed release CAS failed"));
    }
    Ok(true)
}
pub fn input(c: &rusqlite::Connection, runtime: &str, operation: &str) -> Result<Vec<u8>> {
    c.query_row(
        "SELECT body FROM native_managed_operations WHERE runtime_id=?1 AND operation_id=?2",
        params![runtime, operation],
        |r| r.get(0),
    )
    .map_err(Into::into)
}
/// A returned value is API attribution, never native turn/work completion.
/// Late results after invalidation cannot undo Unknown or restore authority.
pub fn returned(
    tx: &Transaction<'_>,
    runtime: &Runtime,
    operation: &str,
    result: &Value,
    now: &str,
) -> Result<Receipt> {
    live(tx, runtime)?;
    let raw = serde_json::to_string(result)?;
    if raw.len() > MAX_RESULT {
        return Err(invalid("managed API result exceeds bounds"));
    }
    let old = get(tx, operation)?
        .filter(|r| r.runtime_id == runtime.id)
        .ok_or_else(|| invalid("managed result intent missing"))?;
    if old.phase == Phase::Returned {
        if old.result.as_ref() == Some(result) {
            return Ok(old);
        }
        return Err(invalid("managed API immutable result conflict"));
    }
    if old.phase != Phase::Dispatched {
        return Err(invalid("managed API result remains unknown"));
    }
    if tx.execute("UPDATE native_managed_operations SET phase='returned',result_json=?2,updated_at=?3 WHERE operation_id=?1 AND phase='dispatched'",params![operation,raw,now])?!=1 {
        return Err(invalid("managed result CAS failed"));
    }
    get(tx, operation)?.ok_or_else(|| invalid("managed API result missing"))
}
pub fn unknown(tx: &Transaction<'_>, runtime: &str, now: &str) -> Result<()> {
    tx.execute("UPDATE native_managed_operations SET phase='unknown',updated_at=?2 WHERE runtime_id=?1 AND phase IN ('prepared','dispatched')",params![runtime,now])?;
    Ok(())
}
/// Restore and daemon boot retain attribution and close every unreturned intent.
/// Never manufacture a new operation UUID or replay a saved body here.
pub fn invalidate_all(c: &rusqlite::Connection, now: &str) -> Result<()> {
    if c.is_autocommit() {
        return Err(invalid("managed invalidation requires owning transaction"));
    }
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_managed_operations')",[],|r|r.get(0))?;
    if exists {
        c.execute("UPDATE native_managed_operations SET phase='unknown',updated_at=?1 WHERE phase IN ('prepared','dispatched')",[now])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn setup() -> (crate::Store, Runtime) {
        let store = crate::Store::open_in_memory().unwrap();
        let c = store.conn();
        c.execute("INSERT INTO crews(id,title,created_at,updated_at) VALUES('managed-crew','synthetic','now','now')",[]).unwrap();
        c.execute("INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('managed-session','managed-crew','worker','acp:claude-managed','now')",[]).unwrap();
        let runtime = Runtime {
            id: id(),
            session: "managed-session".into(),
            generation: id(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "synthetic-boot".into(),
        };
        let tx = c.unchecked_transaction().unwrap();
        crate::native_execution::register(&tx, &runtime).unwrap();
        tx.commit().unwrap();
        (store, runtime)
    }
    #[test]
    fn api_operations_are_one_authority_without_claiming_native_completion() {
        let (store, runtime) = setup();
        let c = store.conn();
        let request = id();
        let tx = c.unchecked_transaction().unwrap();
        let (original, reused) = prepare(
            &tx,
            &runtime,
            "human:fixture",
            true,
            Kind::Prompt,
            &request,
            b"first input",
            "now",
        )
        .unwrap();
        assert!(!reused);
        assert_eq!(original.phase, Phase::Prepared);
        assert!(release(&tx, &runtime, &request, "later").unwrap());
        tx.commit().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(!release(&tx, &runtime, &request, "again").unwrap());
        let result = serde_json::json!({"text":"vendor rewrote input","context":["native context"],"origin":{"kind":"plugin","name":"pendingcrew-managed"}});
        assert_eq!(
            returned(&tx, &runtime, &request, &result, "later")
                .unwrap()
                .phase,
            Phase::Returned
        );
        assert_eq!(
            returned(&tx, &runtime, &request, &result, "duplicate")
                .unwrap()
                .result,
            Some(result)
        );
        tx.commit().unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_execution_receipts", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            0,
            "managed mode must not double-write legacy strong-native intents"
        );
        let tx = c.unchecked_transaction().unwrap();
        let next = id();
        prepare(
            &tx,
            &runtime,
            "human:fixture",
            true,
            Kind::Prompt,
            &next,
            b"second explicit intention",
            "now",
        )
        .unwrap();
        assert!(release(&tx, &runtime, &next, "now").unwrap());
        unknown(&tx, &runtime.id, "disconnect").unwrap();
        assert_eq!(get(&tx, &next).unwrap().unwrap().phase, Phase::Unknown);
        assert!(!release(&tx, &runtime, &next, "retry").unwrap());
        let (old, reused) = prepare(
            &tx,
            &runtime,
            "human:fixture",
            true,
            Kind::Prompt,
            &next,
            b"second explicit intention",
            "retry",
        )
        .unwrap();
        assert!(reused);
        assert_eq!(old.phase, Phase::Unknown);
        assert!(
            prepare(
                &tx,
                &runtime,
                "agent:fixture",
                false,
                Kind::Prompt,
                &id(),
                b"automatic without readiness",
                "now"
            )
            .is_err()
        );
        assert!(
            prepare(
                &tx,
                &runtime,
                "human:fixture",
                true,
                Kind::Prompt,
                &id(),
                b"new explicit Human intention",
                "now"
            )
            .is_ok()
        );
        assert!(
            returned(
                &tx,
                &runtime,
                &next,
                &serde_json::json!({"text":"late"}),
                "late"
            )
            .is_err()
        );
        tx.commit().unwrap();
    }
    #[test]
    fn transaction_failure_restart_and_bounds_never_regrant_an_operation() {
        let (store, runtime) = setup();
        let c = store.conn();
        let request = id();
        {
            let tx = c.unchecked_transaction().unwrap();
            prepare(
                &tx,
                &runtime,
                "human:fixture",
                true,
                Kind::Prompt,
                &request,
                b"private input",
                "now",
            )
            .unwrap();
            release(&tx, &runtime, &request, "now").unwrap();
            // Intentionally no commit: a failed FIFO write releases no permit.
        }
        assert!(get(c, &request).unwrap().is_none());
        let tx = c.unchecked_transaction().unwrap();
        prepare(
            &tx,
            &runtime,
            "human:fixture",
            true,
            Kind::Prompt,
            &request,
            b"private input",
            "now",
        )
        .unwrap();
        release(&tx, &runtime, &request, "now").unwrap();
        tx.commit().unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(
            returned(
                &tx,
                &runtime,
                &request,
                &Value::String("x".repeat(MAX_RESULT)),
                "now"
            )
            .is_err()
        );
        invalidate_all(&tx, "boot").unwrap();
        crate::native_execution::invalidate_unknown(&tx).unwrap();
        tx.commit().unwrap();
        let old = get(c, &request).unwrap().unwrap();
        assert_eq!(old.phase, Phase::Unknown);
        assert!(!format!("{old:?}").contains("private input"));
        let tx = c.unchecked_transaction().unwrap();
        assert!(release(&tx, &runtime, &request, "boot").is_err());
        let (duplicate, reused) = prepare(
            &tx,
            &runtime,
            "human:fixture",
            true,
            Kind::Prompt,
            &request,
            b"private input",
            "boot",
        )
        .unwrap();
        assert!(reused);
        assert_eq!(duplicate.phase, Phase::Unknown);
        assert!(
            prepare(
                &tx,
                &runtime,
                "other-principal",
                true,
                Kind::Prompt,
                &request,
                b"private input",
                "boot"
            )
            .is_err()
        );
        tx.commit().unwrap();
    }
}
