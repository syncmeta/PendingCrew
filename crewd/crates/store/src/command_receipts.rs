//! Single public command receipt ledger. Authentication and original-operation
//! authorization are performed by the caller before any receipt is disclosed.
use crate::{Result, StoreError};
use crewd_proto::commands::{
    CommandPhase, CommandSubmitParams, MAX_COMMAND_PAYLOAD_BYTES, PublicCommandReceipt,
};
use rusqlite::{Connection, OptionalExtension, params};
pub const MAX_PRINCIPAL_RECEIPTS: i64 = 16_384;
pub const MAX_PRINCIPAL_RECEIPT_BYTES: i64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_RECEIPT_BYTES: i64 = 256 * 1024 * 1024;
pub const V14_DDL: &str = r#"
CREATE TABLE public_command_receipts(
 principal TEXT NOT NULL CHECK(length(principal) BETWEEN 1 AND 512),
 request_id TEXT NOT NULL,
 db_id TEXT NOT NULL,
 runtime_binding_id TEXT NOT NULL,
 operation TEXT NOT NULL,
 payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
 payload_sha256 TEXT NOT NULL CHECK(length(payload_sha256)=64),
 phase TEXT NOT NULL CHECK(phase IN ('reserved','committed','admitted','dispatched','launched','written','resized','confirmed','unknown','suppressed','rejected')),
 result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
 result_cap INTEGER NOT NULL CHECK(result_cap BETWEEN 2048 AND 526336),
 historical INTEGER NOT NULL DEFAULT 0 CHECK(historical IN (0,1)),
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 PRIMARY KEY(principal,request_id)
);
CREATE INDEX public_command_unknown ON public_command_receipts(principal,phase,created_at);
CREATE TRIGGER public_command_immutable BEFORE UPDATE OF principal,request_id,db_id,runtime_binding_id,operation,payload_json,payload_sha256,result_cap,created_at ON public_command_receipts BEGIN SELECT RAISE(ABORT,'public command immutable identity'); END;
"#;
fn invalid(s: &str) -> StoreError {
    StoreError::Invalid(s.into())
}
fn phase(value: CommandPhase) -> Result<String> {
    let value = serde_json::to_value(value).map_err(|_| invalid("public command phase invalid"))?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("public command phase invalid"))
}
fn database(c: &Connection, expected: &str) -> Result<()> {
    let actual = crate::meta_get(c, "dbId")?
        .ok_or_else(|| invalid("public command database identity absent"))?;
    if actual != expected {
        return Err(invalid("public command database identity conflict"));
    }
    Ok(())
}
fn input(p: &CommandSubmitParams) -> Result<(String, String)> {
    if !crewd_proto::valid_remote_uuid(&p.request_id)
        || !crewd_proto::valid_remote_uuid(&p.runtime_binding_id)
    {
        return Err(invalid("public command request identity invalid"));
    }
    let json =
        serde_json::to_string(&p.command).map_err(|_| invalid("public command payload invalid"))?;
    if json.len() > MAX_COMMAND_PAYLOAD_BYTES {
        return Err(invalid("public command payload too large"));
    }
    let sha = ring::digest::digest(&ring::digest::SHA256, json.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((json, sha))
}
/// This returns a receipt only in the exact authenticated principal namespace.
/// The server must re-authorize the stored operation/scope before returning it.
pub fn lookup(
    c: &Connection,
    principal: &str,
    db_id: &str,
    request: &str,
) -> Result<Option<PublicCommandReceipt>> {
    database(c, db_id)?;
    if !crewd_proto::valid_remote_uuid(request) {
        return Err(invalid("public command request identity invalid"));
    }
    type StoredReceiptRow = (
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        bool,
        String,
    );
    let row: Option<StoredReceiptRow> = c.query_row(
        "SELECT payload_json,payload_sha256,phase,result_json,created_at,updated_at,historical,runtime_binding_id FROM public_command_receipts WHERE principal=?1 AND request_id=?2 AND db_id=?3",
        params![principal,request,db_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
    row.map(
        |(payload, hash, p, result, created, updated, historical, runtime_binding_id)| {
            Ok(PublicCommandReceipt {
                runtime_binding_id,
                request_id: request.into(),
                db_id: db_id.into(),
                command: serde_json::from_str(&payload)
                    .map_err(|_| invalid("stored command payload corrupt"))?,
                payload_sha256: hash,
                phase: serde_json::from_value(serde_json::Value::String(p))
                    .map_err(|_| invalid("stored command phase corrupt"))?,
                result: result
                    .map(|r| {
                        serde_json::from_str(&r)
                            .map_err(|_| invalid("stored command result corrupt"))
                    })
                    .transpose()?,
                created_at: created,
                updated_at: updated,
                historical,
            })
        },
    )
    .transpose()
}
#[derive(Debug)]
pub enum Reserved {
    New(PublicCommandReceipt),
    Existing(PublicCommandReceipt),
}
/// Must join the same transaction as SQL facts or durable external admission.
/// Identical repeats return attribution only; they never dispatch an executor.
pub fn reserve(
    c: &Connection,
    principal: &str,
    p: &CommandSubmitParams,
    at: &str,
) -> Result<Reserved> {
    database(c, &p.db_id)?;
    if principal.is_empty() || principal.len() > 512 {
        return Err(invalid("public command principal invalid"));
    }
    let (payload, hash) = input(p)?;
    if let Some(old) = lookup(c, principal, &p.db_id, &p.request_id)? {
        if old.payload_sha256 != hash
            || old.command != p.command
            || old.runtime_binding_id != p.runtime_binding_id
        {
            return Err(invalid("public command immutable payload conflict"));
        }
        return Ok(Reserved::Existing(old));
    }
    // Capacity refusal preserves all active/unknown attribution. We never prune
    // an identity into not-found or silently recreate a settled external input.
    let (count,own_bytes,total_bytes):(i64,i64,i64)=c.query_row(
        "SELECT SUM(CASE WHEN principal=?1 THEN 1 ELSE 0 END), COALESCE(SUM(CASE WHEN principal=?1 THEN length(CAST(payload_json AS BLOB))+result_cap ELSE 0 END),0), COALESCE(SUM(length(CAST(payload_json AS BLOB))+result_cap),0) FROM public_command_receipts",
        [principal],|r|Ok((r.get::<_,Option<i64>>(0)?.unwrap_or(0),r.get(1)?,r.get(2)?)))?;
    let result_cap = 2 * payload.len() as i64 + 2048;
    if count >= MAX_PRINCIPAL_RECEIPTS
        || own_bytes + payload.len() as i64 + result_cap > MAX_PRINCIPAL_RECEIPT_BYTES
        || total_bytes + payload.len() as i64 + result_cap > MAX_TOTAL_RECEIPT_BYTES
    {
        return Err(invalid("public command receipt capacity reached"));
    }
    c.execute("INSERT INTO public_command_receipts(principal,request_id,db_id,runtime_binding_id,operation,payload_json,payload_sha256,result_cap,phase,created_at,updated_at) VALUES(?1,?2,?3,?9,?4,?5,?6,?8,'reserved',?7,?7)",params![principal,p.request_id,p.db_id,p.command.method(),payload,hash,at,result_cap,p.runtime_binding_id])?;
    Ok(Reserved::New(
        lookup(c, principal, &p.db_id, &p.request_id)?
            .ok_or_else(|| invalid("reserved public command missing"))?,
    ))
}
/// A compare-and-set transition settles only the exact previous obligation.
/// Unknown/suppressed/historical receipts never enter a dispatch phase again.
pub fn transition(
    c: &Connection,
    principal: &str,
    db: &str,
    request: &str,
    phases: (CommandPhase, CommandPhase),
    result: Option<&serde_json::Value>,
    at: &str,
) -> Result<PublicCommandReceipt> {
    let (from, to) = phases;
    database(c, db)?;
    let legal = matches!(
        (from, to),
        (
            CommandPhase::Reserved,
            CommandPhase::Committed
                | CommandPhase::Admitted
                | CommandPhase::Rejected
                | CommandPhase::Suppressed
                | CommandPhase::Unknown
        ) | (
            CommandPhase::Admitted,
            CommandPhase::Dispatched | CommandPhase::Suppressed | CommandPhase::Unknown
        ) | (
            CommandPhase::Dispatched,
            CommandPhase::Launched
                | CommandPhase::Written
                | CommandPhase::Resized
                | CommandPhase::Confirmed
                | CommandPhase::Unknown
                | CommandPhase::Rejected
        )
    );
    if !legal {
        return Err(invalid("public command phase transition denied"));
    }
    let result = result
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| invalid("public command result invalid"))?;
    let result_cap:i64=c.query_row("SELECT result_cap FROM public_command_receipts WHERE principal=?1 AND request_id=?2 AND db_id=?3",params![principal,request,db],|r|r.get(0))?;
    if result.as_ref().is_some_and(|r| r.len() as i64 > result_cap) {
        return Err(invalid("public command result reservation exceeded"));
    }
    if result
        .as_ref()
        .is_some_and(|r| r.len() > MAX_COMMAND_PAYLOAD_BYTES * 2)
    {
        return Err(invalid("public command result too large"));
    }
    if c.execute("UPDATE public_command_receipts SET phase=?4,result_json=?5,updated_at=?6 WHERE principal=?1 AND request_id=?2 AND db_id=?3 AND phase=?7 AND historical=0",params![principal,request,db,phase(to)?,result,at,phase(from)?])?!=1 {return Err(invalid("public command phase CAS conflict"));}
    lookup(c, principal, db, request)?.ok_or_else(|| invalid("transitioned public command missing"))
}
/// Record the SQL session allocation beside its transaction. This is not a
/// launched/completed result, and cannot be rewritten or used to dispatch twice.
pub fn record_registered(
    c: &Connection,
    principal: &str,
    db: &str,
    request: &str,
    session: &str,
    at: &str,
) -> Result<()> {
    database(c, db)?;
    let result = serde_json::json!({"sessionRegistered":true,"sessionId":session}).to_string();
    if c.execute("UPDATE public_command_receipts SET result_json=?4,updated_at=?5 WHERE principal=?1 AND request_id=?2 AND db_id=?3 AND phase='dispatched' AND historical=0 AND result_json IS NULL AND result_cap>=length(CAST(?4 AS BLOB))", params![principal,request,db,result,at])? != 1 {
        return Err(invalid("public session allocation CAS conflict"));
    }
    Ok(())
}

/// Bounded namespace page. The server checks each stored operation/scope before
/// disclosure and emits small summaries, not concatenated original body frames.
pub fn list_unknown(
    c: &Connection,
    principal: &str,
    db: &str,
    after: Option<&str>,
    limit: u16,
) -> Result<Vec<PublicCommandReceipt>> {
    database(c, db)?;
    if !(1..=50).contains(&limit) || after.is_some_and(|s| !crewd_proto::valid_remote_uuid(s)) {
        return Err(invalid("public command unknown page invalid"));
    }
    let mut stmt=c.prepare("SELECT request_id FROM public_command_receipts WHERE principal=?1 AND db_id=?2 AND phase IN ('unknown','reserved','admitted','dispatched') AND (?3 IS NULL OR request_id>?3) ORDER BY request_id LIMIT ?4")?;
    let ids = stmt
        .query_map(params![principal, db, after, i64::from(limit) + 1], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| {
            lookup(c, principal, db, &id)?
                .ok_or_else(|| invalid("public command unknown page lost receipt"))
        })
        .collect()
}
/// No pending external command is automatically replayed after process loss.
pub fn interrupted(c: &Connection, at: &str) -> Result<()> {
    c.execute("UPDATE public_command_receipts SET phase='unknown',updated_at=?1 WHERE phase IN ('reserved','admitted','dispatched')",[at])?;
    Ok(())
}
/// Rollback/import can erase observations of external effects. Keep attribution,
/// but no copied external result is promoted to current runtime completion.
pub fn restored(c: &Connection, at: &str) -> Result<()> {
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='public_command_receipts')",[],|r|r.get(0))?;
    if !exists {
        return Ok(());
    }
    c.execute("UPDATE public_command_receipts SET historical=1,phase=CASE WHEN phase='committed' THEN phase WHEN phase IN ('rejected','suppressed') THEN phase ELSE 'unknown' END,updated_at=?1",[at])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{CrewCreateParams, commands::PublicCommand};
    fn setup() -> crate::Store {
        crate::Store::open_in_memory().unwrap()
    }
    fn request(s: &crate::Store) -> CommandSubmitParams {
        CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_binding_id: uuid::Uuid::new_v4().to_string(),
            db_id: crate::meta_get(s.conn(), "dbId").unwrap().unwrap(),
            command: PublicCommand::CrewCreate(CrewCreateParams {
                title: "single creation".into(),
                working_directory: "/private/tmp".into(),
                parent_id: None,
            }),
        }
    }
    #[test]
    fn same_identity_is_attribution_and_changed_payload_never_dispatches() {
        let s = setup();
        let p = request(&s);
        assert!(matches!(
            reserve(s.conn(), "kernel-human-owner", &p, "t1").unwrap(),
            Reserved::New(_)
        ));
        transition(
            s.conn(),
            "kernel-human-owner",
            &p.db_id,
            &p.request_id,
            (CommandPhase::Reserved, CommandPhase::Committed),
            Some(&serde_json::json!({"crewId":"real-created-fact"})),
            "t2",
        )
        .unwrap();
        let old = match reserve(s.conn(), "kernel-human-owner", &p, "t3").unwrap() {
            Reserved::Existing(r) => r,
            _ => panic!("duplicate admitted again"),
        };
        assert_eq!(old.phase, CommandPhase::Committed);
        let mut changed = p.clone();
        if let PublicCommand::CrewCreate(c) = &mut changed.command {
            c.title = "different payload".into();
        }
        assert!(reserve(s.conn(), "kernel-human-owner", &changed, "t4").is_err());
        assert!(
            lookup(
                s.conn(),
                "another-authenticated-principal",
                &p.db_id,
                &p.request_id
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            lookup(s.conn(), "kernel-human-owner", &p.db_id, &p.request_id).unwrap(),
            Some(old)
        );
    }
    #[test]
    fn unknown_and_restored_external_results_cannot_reenter_dispatch() {
        let s = setup();
        let p = request(&s);
        reserve(s.conn(), "paired-grant", &p, "t1").unwrap();
        transition(
            s.conn(),
            "paired-grant",
            &p.db_id,
            &p.request_id,
            (CommandPhase::Reserved, CommandPhase::Admitted),
            None,
            "t2",
        )
        .unwrap();
        transition(
            s.conn(),
            "paired-grant",
            &p.db_id,
            &p.request_id,
            (CommandPhase::Admitted, CommandPhase::Dispatched),
            None,
            "t3",
        )
        .unwrap();
        interrupted(s.conn(), "t4").unwrap();
        assert_eq!(
            lookup(s.conn(), "paired-grant", &p.db_id, &p.request_id)
                .unwrap()
                .unwrap()
                .phase,
            CommandPhase::Unknown
        );
        assert!(
            transition(
                s.conn(),
                "paired-grant",
                &p.db_id,
                &p.request_id,
                (CommandPhase::Unknown, CommandPhase::Admitted),
                None,
                "t5"
            )
            .is_err()
        );
        assert!(matches!(
            reserve(s.conn(), "paired-grant", &p, "t6").unwrap(),
            Reserved::Existing(_)
        ));
        restored(s.conn(), "t7").unwrap();
        assert!(
            lookup(s.conn(), "paired-grant", &p.db_id, &p.request_id)
                .unwrap()
                .unwrap()
                .historical
        );
        assert!(
            transition(
                s.conn(),
                "paired-grant",
                &p.db_id,
                &p.request_id,
                (CommandPhase::Reserved, CommandPhase::Admitted),
                None,
                "t8"
            )
            .is_err()
        );
    }
    #[test]
    fn same_transaction_rollback_removes_reserved_identity_and_sql_fact() {
        let mut s = setup();
        let p = request(&s);
        let result: crate::Result<()> = s.with_tx(|c| {
            reserve(c, "kernel-human-owner", &p, "t1")?;
            c.execute(
                "INSERT INTO meta(key,value) VALUES('synthetic-command-business','one')",
                [],
            )?;
            Err(invalid("synthetic later business failure"))
        });
        assert!(result.is_err());
        assert!(
            lookup(s.conn(), "kernel-human-owner", &p.db_id, &p.request_id)
                .unwrap()
                .is_none()
        );
        assert!(
            crate::meta_get(s.conn(), "synthetic-command-business")
                .unwrap()
                .is_none()
        );
    }
}
