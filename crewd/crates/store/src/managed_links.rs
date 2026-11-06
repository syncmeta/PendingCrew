//! Only new manual/column links. Authoritative ledger fields remain adapters.
use crate::{Result, StoreError, managed_objects, managed_tables_v2 as tables};
use crewd_proto::{managed_objects::Scope, managed_tables_v2::*};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
pub const DDL: &str = r#"
CREATE TABLE managed_link_clock(id INTEGER PRIMARY KEY CHECK(id=1),value INTEGER NOT NULL CHECK(value>=0),secret BLOB NOT NULL CHECK(length(secret)=32));
INSERT INTO managed_link_clock VALUES(1,0,randomblob(32));
CREATE TABLE managed_link_stamps(source_key TEXT NOT NULL,owner_crew TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,revision INTEGER NOT NULL CHECK(revision>0),PRIMARY KEY(source_key,owner_crew)) WITHOUT ROWID;
CREATE TABLE managed_links(
 id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),owner_crew TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 source_key TEXT NOT NULL,source_json TEXT NOT NULL CHECK(json_valid(source_json)),origin TEXT NOT NULL,
 source_number INTEGER REFERENCES object_refs(number) ON DELETE CASCADE,source_artifact TEXT REFERENCES managed_artifacts(reference) ON DELETE RESTRICT,
 target_key TEXT NOT NULL,target_json TEXT NOT NULL CHECK(json_valid(target_json)),
 target_number INTEGER REFERENCES object_refs(number) ON DELETE CASCADE,target_artifact TEXT REFERENCES managed_artifacts(reference) ON DELETE RESTRICT,
 target_crew TEXT REFERENCES crews(id) ON DELETE CASCADE,target_session TEXT REFERENCES sessions(id) ON DELETE CASCADE,
 target_channel TEXT REFERENCES channels(id) ON DELETE CASCADE,target_message TEXT REFERENCES messages(id) ON DELETE CASCADE,
 created_at TEXT NOT NULL,UNIQUE(source_key,owner_crew,origin,target_key)
);
CREATE INDEX managed_artifact_adapter_identity ON managed_artifact_edges(target_number,reference,relation,id);
CREATE INDEX managed_links_outgoing ON managed_links(source_key,id);
CREATE INDEX managed_links_incoming ON managed_links(target_key,id);
CREATE INDEX managed_links_target_object ON managed_links(target_number,source_number);
CREATE TABLE managed_v2_receipts(
 owner_crew TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,principal TEXT NOT NULL,request_id TEXT NOT NULL,
 method TEXT NOT NULL,digest TEXT NOT NULL CHECK(length(digest)=64),input_json TEXT CHECK(input_json IS NULL OR json_valid(input_json)),
 result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN(0,1)),
 PRIMARY KEY(owner_crew,principal,request_id),CHECK((retired=0 AND input_json IS NOT NULL AND result_json IS NOT NULL) OR (retired=1 AND input_json IS NULL AND result_json IS NULL))
) WITHOUT ROWID;
CREATE TABLE managed_link_events(id TEXT PRIMARY KEY,owner_crew TEXT NOT NULL,principal TEXT NOT NULL,request_id TEXT NOT NULL,method TEXT NOT NULL,digest TEXT NOT NULL,created_at TEXT NOT NULL,FOREIGN KEY(owner_crew,principal,request_id) REFERENCES managed_v2_receipts(owner_crew,principal,request_id) ON DELETE CASCADE) WITHOUT ROWID;
CREATE TABLE managed_table_schema_events(event_id TEXT PRIMARY KEY REFERENCES managed_link_events(id) ON DELETE CASCADE,table_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,before_json TEXT NOT NULL CHECK(json_valid(before_json)),after_json TEXT NOT NULL CHECK(json_valid(after_json)));
CREATE TABLE managed_v2_receipt_objects(owner_crew TEXT NOT NULL,principal TEXT NOT NULL,request_id TEXT NOT NULL,number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
 PRIMARY KEY(owner_crew,principal,request_id,number),FOREIGN KEY(owner_crew,principal,request_id) REFERENCES managed_v2_receipts(owner_crew,principal,request_id) ON DELETE CASCADE) WITHOUT ROWID;
DROP TRIGGER managed_artifact_edge_gc;
DROP TRIGGER managed_artifact_claim_gc;
CREATE TRIGGER managed_artifact_edge_gc AFTER DELETE ON managed_artifact_edges BEGIN
 DELETE FROM managed_artifacts WHERE reference=OLD.reference
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_edges WHERE reference=OLD.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_claims WHERE reference=OLD.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_links WHERE source_artifact=OLD.reference OR target_artifact=OLD.reference);
END;
CREATE TRIGGER managed_artifact_claim_gc AFTER DELETE ON managed_artifact_claims BEGIN
 DELETE FROM managed_artifacts WHERE reference=OLD.reference
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_edges WHERE reference=OLD.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_claims WHERE reference=OLD.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_links WHERE source_artifact=OLD.reference OR target_artifact=OLD.reference);
END;
CREATE TRIGGER managed_links_artifact_gc AFTER DELETE ON managed_links BEGIN
 DELETE FROM managed_artifacts WHERE reference IN(OLD.source_artifact,OLD.target_artifact)
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_edges WHERE reference=managed_artifacts.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_claims WHERE reference=managed_artifacts.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_links WHERE source_artifact=managed_artifacts.reference OR target_artifact=managed_artifacts.reference);
END;
CREATE TABLE managed_v2_receipt_knowledge(owner_crew TEXT NOT NULL,principal TEXT NOT NULL,request_id TEXT NOT NULL,target_key TEXT NOT NULL,
 PRIMARY KEY(owner_crew,principal,request_id,target_key),FOREIGN KEY(owner_crew,principal,request_id) REFERENCES managed_v2_receipts(owner_crew,principal,request_id) ON DELETE CASCADE) WITHOUT ROWID;
CREATE TABLE managed_link_deleted_rows(number INTEGER PRIMARY KEY REFERENCES managed_rows(number) ON DELETE CASCADE);
CREATE TABLE managed_link_deleted_sources(source_key TEXT NOT NULL,owner_crew TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,PRIMARY KEY(source_key,owner_crew)) WITHOUT ROWID;
CREATE TRIGGER managed_v2_retire_object BEFORE DELETE ON object_refs BEGIN
 UPDATE managed_v2_receipts SET input_json=NULL,result_json=NULL,retired=1 WHERE (owner_crew,principal,request_id) IN(SELECT owner_crew,principal,request_id FROM managed_v2_receipt_objects WHERE number=OLD.number);
 INSERT OR IGNORE INTO managed_link_deleted_rows SELECT source_number FROM managed_links WHERE target_number=OLD.number AND origin LIKE 'column:%' AND source_number!=OLD.number;
 INSERT OR IGNORE INTO managed_link_deleted_sources SELECT source_key,owner_crew FROM managed_links WHERE target_number=OLD.number AND origin='manual';
END;
CREATE TRIGGER managed_v2_message_deleted BEFORE DELETE ON messages BEGIN
 INSERT OR IGNORE INTO managed_link_deleted_rows SELECT source_number FROM managed_links WHERE target_message=OLD.id AND origin LIKE 'column:%';
 INSERT OR IGNORE INTO managed_link_deleted_sources SELECT source_key,owner_crew FROM managed_links WHERE target_message=OLD.id AND origin='manual';
END;
CREATE TRIGGER managed_v2_channel_deleted BEFORE DELETE ON channels BEGIN
 INSERT OR IGNORE INTO managed_link_deleted_rows SELECT source_number FROM managed_links WHERE target_channel=OLD.id AND origin LIKE 'column:%';
 INSERT OR IGNORE INTO managed_link_deleted_sources SELECT source_key,owner_crew FROM managed_links WHERE target_channel=OLD.id AND origin='manual';
END;
CREATE TRIGGER managed_v2_session_deleted BEFORE DELETE ON sessions BEGIN
 INSERT OR IGNORE INTO managed_link_deleted_rows SELECT source_number FROM managed_links WHERE target_session=OLD.id AND origin LIKE 'column:%';
 INSERT OR IGNORE INTO managed_link_deleted_sources SELECT source_key,owner_crew FROM managed_links WHERE target_session=OLD.id AND origin='manual';
END;
CREATE TRIGGER managed_v2_crew_deleted BEFORE DELETE ON crews BEGIN
 INSERT OR IGNORE INTO managed_link_deleted_rows SELECT source_number FROM managed_links WHERE target_crew=OLD.id AND origin LIKE 'column:%' AND owner_crew!=OLD.id;
 INSERT OR IGNORE INTO managed_link_deleted_sources SELECT source_key,owner_crew FROM managed_links WHERE target_crew=OLD.id AND origin='manual';
END;
"#;
fn bad() -> StoreError {
    StoreError::Invalid("invalid managed link or original receipt".into())
}
pub fn json<T: Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}
pub fn hash(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2)
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| bad()))
        .collect()
}
fn secret(c: &Connection) -> Result<Vec<u8>> {
    let v: Vec<u8> = c.query_row(
        "SELECT secret FROM managed_link_clock WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    if v.len() != 32 {
        return Err(bad());
    }
    Ok(v)
}
pub fn mac(c: &Connection, domain: &str, bytes: &[u8]) -> Result<String> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, &secret(c)?);
    let mut msg = domain.as_bytes().to_vec();
    msg.push(0);
    msg.extend_from_slice(bytes);
    Ok(hex(hmac::sign(&key, &msg).as_ref()))
}
pub fn seal(c: &Connection, domain: &str, bytes: &[u8]) -> Result<String> {
    let k = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &secret(c)?).map_err(|_| bad())?,
    );
    let mut nonce = [0; 12];
    SystemRandom::new().fill(&mut nonce).map_err(|_| bad())?;
    let mut out = bytes.to_vec();
    k.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(domain.as_bytes()),
        &mut out,
    )
    .map_err(|_| bad())?;
    let mut all = nonce.to_vec();
    all.extend(out);
    Ok(hex(&all))
}
pub fn open(c: &Connection, domain: &str, token: &str) -> Result<Vec<u8>> {
    if token.len() > 16384 {
        return Err(bad());
    }
    let b = unhex(token)?;
    if b.len() < 28 {
        return Err(bad());
    }
    let nonce: [u8; 12] = b[..12].try_into().map_err(|_| bad())?;
    let mut out = b[12..].to_vec();
    let k = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &secret(c)?).map_err(|_| bad())?,
    );
    Ok(k.open_in_place(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(domain.as_bytes()),
        &mut out,
    )
    .map_err(|_| bad())?
    .to_vec())
}
pub fn intent<T: Serialize>(
    method: &str,
    scope: &Scope,
    crew: &str,
    uuid: &str,
    operation: &T,
) -> Result<String> {
    let node = serde_json::json!({"method":method,"scope":scope,"crewId":crew,"requestId":uuid,"operation":operation});
    // The physical receipt namespace is shared; the codec domain is not.
    // Dispatch by the recorded method before interpreting either version.
    let mut b = if method == crewd_proto::managed_tables_v3::SUBMIT {
        crewd_proto::managed_tables_v3::INTENT_DOMAIN.to_vec()
    } else {
        b"managed-v2-intent.v1\0".to_vec()
    };
    encode(&node, &mut b)?;
    Ok(hash(&b))
}
pub fn encode(n: &serde_json::Value, b: &mut Vec<u8>) -> Result<()> {
    use serde_json::Value;
    let len = |b: &mut Vec<u8>, n: usize| b.extend_from_slice(&(n as u64).to_be_bytes());
    match n {
        Value::Null => b.push(0),
        Value::Bool(false) => b.push(1),
        Value::Bool(true) => b.push(2),
        Value::String(s) => {
            b.push(0x10);
            len(b, s.len());
            b.extend_from_slice(s.as_bytes())
        }
        Value::Number(n) => {
            b.push(0x11);
            b.extend_from_slice(&n.as_u64().ok_or_else(bad)?.to_be_bytes())
        }
        Value::Array(v) => {
            b.push(0x20);
            len(b, v.len());
            for n in v {
                encode(n, b)?
            }
        }
        Value::Object(m) => {
            b.push(0x21);
            len(b, m.len());
            let mut keys = m.keys().collect::<Vec<_>>();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            for key in keys {
                encode(&Value::String(key.clone()), b)?;
                if key == "value" && m.get("type").and_then(Value::as_str) == Some("real") {
                    let real = m[key].as_f64().filter(|n| n.is_finite()).ok_or_else(bad)?;
                    b.push(0x12);
                    b.extend_from_slice(&real.to_bits().to_be_bytes())
                } else {
                    encode(&m[key], b)?
                }
            }
        }
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct Edge {
    pub id: i64,
    pub owner: String,
    pub source: SourceRef,
    pub target: TargetRef,
    pub origin: String,
    pub created_at: String,
}
pub fn source_key(v: &SourceRef) -> Result<String> {
    json(v)
}
pub fn target_key(v: &TargetRef) -> Result<String> {
    match v {
        TargetRef::Document { source } | TargetRef::Knowledge { source } => {
            let mut context = serde_json::to_value(&source.context)?;
            context
                .as_object_mut()
                .ok_or_else(bad)?
                .remove("generation");
            json(
                &serde_json::json!({"kind":"sqliteKnowledge","context":context,"reference":source.reference}),
            )
        }
        _ => json(v),
    }
}
pub fn source_number(s: &SourceRef) -> Result<Option<i64>> {
    Ok(match s {
        SourceRef::Work { id } => Some(tables::number(id, 'W')?),
        SourceRef::Requirement { id } => Some(tables::number(id, '#')?),
        SourceRef::Ask { id } => Some(tables::number(id, 'H')?),
        SourceRef::Table { id } => Some(tables::number(id, 'T')?),
        SourceRef::Row { id } => Some(tables::number(id, 'O')?),
        SourceRef::Decision { id } => Some(tables::number(id, 'D')?),
        SourceRef::Risk { id } => Some(tables::number(id, 'R')?),
        SourceRef::Lesson { id } => Some(tables::number(id, 'L')?),
        SourceRef::Artifact { .. } => None,
    })
}
pub fn target_number(t: &TargetRef) -> Result<Option<i64>> {
    Ok(match t {
        TargetRef::Work { id } => Some(tables::number(id, 'W')?),
        TargetRef::Requirement { id } => Some(tables::number(id, '#')?),
        TargetRef::Ask { id } => Some(tables::number(id, 'H')?),
        TargetRef::Table { id } => Some(tables::number(id, 'T')?),
        TargetRef::Row { id } => Some(tables::number(id, 'O')?),
        TargetRef::Decision { id } => Some(tables::number(id, 'D')?),
        TargetRef::Risk { id } => Some(tables::number(id, 'R')?),
        TargetRef::Lesson { id } => Some(tables::number(id, 'L')?),
        _ => None,
    })
}
fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, String, String, String, String, String)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}
fn edge(r: (i64, String, String, String, String, String)) -> Result<Edge> {
    let (id, owner, source, target, origin, created_at) = r;
    let source: SourceRef = serde_json::from_str(&source)?;
    let target: TargetRef = serde_json::from_str(&target)?;
    if id <= 0
        || owner.is_empty()
        || created_at.is_empty()
        || !(origin == "manual"
            || origin
                .strip_prefix("column:")
                .is_some_and(|p| tables::number(p, 'p').is_ok()))
    {
        return Err(bad());
    }
    Ok(Edge {
        id,
        owner,
        source,
        target,
        origin,
        created_at,
    })
}
pub fn outgoing(c: &Connection, source: &SourceRef) -> Result<Vec<Edge>> {
    let mut q=c.prepare("SELECT id,owner_crew,source_json,target_json,origin,created_at FROM managed_links WHERE source_key=?1 ORDER BY id")?;
    q.query_map([source_key(source)?], read)?
        .map(|r| edge(r?))
        .collect()
}
pub fn incoming(c: &Connection, target: &TargetRef, after: i64, limit: u16) -> Result<Vec<Edge>> {
    let mut q=c.prepare("SELECT id,owner_crew,source_json,target_json,origin,created_at FROM managed_links WHERE target_key=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
    q.query_map(params![target_key(target)?, after, limit], read)?
        .map(|r| edge(r?))
        .collect()
}
pub fn by_id(c: &Connection, id: i64) -> Result<Option<Edge>> {
    c.query_row("SELECT id,owner_crew,source_json,target_json,origin,created_at FROM managed_links WHERE id=?1",[id],read).optional()?.map(edge).transpose()
}
pub fn link(
    c: &Connection,
    owner: &str,
    source: &SourceRef,
    target: &TargetRef,
    origin: &str,
    now: &str,
) -> Result<(i64, bool)> {
    managed_objects::transaction(c)?;
    let sk = source_key(source)?;
    let tk = target_key(target)?;
    if let Some(id)=c.query_row("SELECT id FROM managed_links WHERE source_key=?1 AND owner_crew=?2 AND origin=?3 AND target_key=?4",params![sk,owner,origin,tk],|r|r.get(0)).optional()?{return Ok((id,false))}
    let sa = match source {
        SourceRef::Artifact { reference, .. } => Some(json(reference)?),
        _ => None,
    };
    let ta = match target {
        TargetRef::Artifact { reference } => Some(json(reference)?),
        _ => None,
    };
    let tc = match target {
        TargetRef::Crew { id } => Some(id),
        _ => None,
    };
    let ts = match target {
        TargetRef::Session { id } => Some(id),
        _ => None,
    };
    let tch = match target {
        TargetRef::Channel { id } => Some(id),
        _ => None,
    };
    let tm = match target {
        TargetRef::Message { id } => Some(id),
        _ => None,
    };
    c.execute("INSERT INTO managed_links(owner_crew,source_key,source_json,origin,source_number,source_artifact,target_key,target_json,target_number,target_artifact,target_crew,target_session,target_channel,target_message,created_at) VALUES(?1,?2,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![owner,sk,origin,source_number(source)?,sa,tk,json(target)?,target_number(target)?,ta,tc,ts,tch,tm,now])?;
    Ok((c.last_insert_rowid(), true))
}
pub fn remove(c: &Connection, id: i64) -> Result<()> {
    managed_objects::transaction(c)?;
    if c.execute("DELETE FROM managed_links WHERE id=?1", [id])? != 1 {
        return Err(bad());
    }
    Ok(())
}
pub fn stamp(c: &Connection, s: &SourceRef, crew: &str) -> Result<i64> {
    Ok(c.query_row(
        "SELECT revision FROM managed_link_stamps WHERE source_key=?1 AND owner_crew=?2",
        params![source_key(s)?, crew],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or(0))
}
pub fn advance(c: &Connection, s: &SourceRef, crew: &str) -> Result<i64> {
    managed_objects::transaction(c)?;
    let n = stamp(c, s, crew)?.checked_add(1).ok_or_else(bad)?;
    let old_clock: i64 =
        c.query_row("SELECT value FROM managed_link_clock WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let clock = old_clock.checked_add(1).ok_or_else(bad)?.max(n);
    c.execute("UPDATE managed_link_clock SET value=?1 WHERE id=1", [clock])?;
    c.execute("INSERT INTO managed_link_stamps VALUES(?1,?2,?3) ON CONFLICT(source_key,owner_crew) DO UPDATE SET revision=excluded.revision",params![source_key(s)?,crew,n])?;
    Ok(n)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub method: String,
    pub digest: String,
    pub input: serde_json::Value,
    pub result: serde_json::Value,
    pub seal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
}
fn checksum(s: &Saved) -> Result<String> {
    if let Some(principal) = &s.principal {
        return Ok(hash(&serde_json::to_vec(&(
            &s.method, &s.digest, &s.input, &s.result, principal,
        ))?));
    }
    Ok(hash(&serde_json::to_vec(&(
        &s.method, &s.digest, &s.input, &s.result,
    ))?))
}
pub enum ReadReceipt {
    Active(Saved),
    Retired,
}
/// The codec discriminator survives payload retirement. Recovery routes must
/// not interpret another codec's tombstone as their own absent original.
pub fn receipt_method(
    c: &Connection,
    crew: &str,
    principal: &str,
    uuid: &str,
) -> Result<Option<String>> {
    Ok(c.query_row("SELECT method FROM managed_v2_receipts WHERE owner_crew=?1 AND principal=?2 AND request_id=?3",params![crew,principal,uuid],|r|r.get(0)).optional()?)
}
pub fn receipt(
    c: &Connection,
    crew: &str,
    principal: &str,
    uuid: &str,
) -> Result<Option<ReadReceipt>> {
    let r=c.query_row("SELECT method,digest,input_json,result_json,retired FROM managed_v2_receipts WHERE owner_crew=?1 AND principal=?2 AND request_id=?3",params![crew,principal,uuid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,bool>(4)?))).optional()?;
    r.map(|(method, digest, input, result, retired)| {
        if retired {
            return Ok(ReadReceipt::Retired);
        }
        let raw=result.ok_or_else(bad)?;
        let encoded:serde_json::Value=serde_json::from_str(&raw)?;
        if method!=crewd_proto::managed_tables_v3::SUBMIT&&encoded.get("principal").is_some(){return Err(bad())}
        let saved: Saved = serde_json::from_value(encoded)?;
        if saved.method != method
            || saved.digest != digest
            || saved.input != serde_json::from_str::<serde_json::Value>(&input.ok_or_else(bad)?)?
            || saved.seal != checksum(&saved)?
        {
            return Err(bad());
        }
        let v3=method==crewd_proto::managed_tables_v3::SUBMIT;
        if (v3&&saved.principal.as_deref()!=Some(principal))||(!v3&&saved.principal.is_some()){return Err(bad())}
        let scope: Scope =
            serde_json::from_value(saved.input.get("scope").cloned().ok_or_else(bad)?)?;
        if saved.input.get("crewId").and_then(|v| v.as_str()) != Some(crew)
            || saved.input.get("requestId").and_then(|v| v.as_str()) != Some(uuid)
            || intent(
                &saved.method,
                &scope,
                crew,
                uuid,
                saved.input.get("operation").ok_or_else(bad)?,
            )? != digest
        {
            return Err(bad());
        }
        if v3{
            let input:crewd_proto::managed_tables_v3::Submit=serde_json::from_value(saved.input.clone())?;
            let receipt:crewd_proto::managed_tables_v3::Receipt=serde_json::from_value(saved.result.clone())?;
            crate::managed_tables_v3::validate_receipt(&input,&receipt)?;
            if let Some(event)=&receipt.event_id{
                let exact:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM managed_link_events WHERE id=?1 AND owner_crew=?2 AND principal=?3 AND request_id=?4 AND method=?5 AND digest=?6 AND created_at=?7)",params![event,crew,principal,uuid,method,digest,receipt.recorded_at],|r|r.get(0))?;
                if !exact{return Err(bad())}
                let (source,before,after)=crate::managed_tables_v3::event_witness(&receipt)?;
                let action=serde_json::to_value(receipt.operation_kind)?;
                let audit_exact:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM managed_table_audit WHERE event_id=?1 AND source_number=?2 AND action=?3 AND recorded_at=?4 AND original_revision IS ?5 AND result_revision=?6)",params![event,source,action.as_str().ok_or_else(bad)?,receipt.recorded_at,before,after],|r|r.get(0))?;
                if !audit_exact{return Err(bad())}
            }
        }
        Ok(ReadReceipt::Active(saved))
    })
    .transpose()
}
pub struct ReceiptInput<'a, I, R> {
    pub crew: &'a str,
    pub principal: &'a str,
    pub uuid: &'a str,
    pub method: &'a str,
    pub digest: &'a str,
    pub input: &'a I,
    pub result: &'a R,
    pub objects: &'a [i64],
}
pub fn record<I: Serialize, R: Serialize>(
    c: &Connection,
    receipt: ReceiptInput<'_, I, R>,
) -> Result<()> {
    let ReceiptInput {
        crew,
        principal,
        uuid,
        method,
        digest,
        input,
        result,
        objects,
    } = receipt;
    managed_objects::transaction(c)?;
    let mut s = Saved {
        method: method.into(),
        digest: digest.into(),
        input: serde_json::to_value(input)?,
        result: serde_json::to_value(result)?,
        seal: String::new(),
        principal: (method == crewd_proto::managed_tables_v3::SUBMIT).then(|| principal.to_owned()),
    };
    s.seal = checksum(&s)?;
    c.execute("INSERT INTO managed_v2_receipts(owner_crew,principal,request_id,method,digest,input_json,result_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![crew,principal,uuid,method,digest,json(&s.input)?,json(&s)?])?;
    if let Some(event) = s.result.get("eventId").and_then(|v| v.as_str()) {
        if !crewd_proto::valid_remote_uuid(event) {
            return Err(bad());
        }
        let at = s
            .result
            .get("recordedAt")
            .and_then(|v| v.as_str())
            .ok_or_else(bad)?;
        c.execute(
            "INSERT INTO managed_link_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![event, crew, principal, uuid, method, digest, at],
        )?;
    }
    for target in knowledge_targets(&s.input) {
        c.execute(
            "INSERT OR IGNORE INTO managed_v2_receipt_knowledge VALUES(?1,?2,?3,?4)",
            params![crew, principal, uuid, target_key(&target)?],
        )?;
    }
    for n in objects {
        c.execute(
            "INSERT OR IGNORE INTO managed_v2_receipt_objects VALUES(?1,?2,?3,?4)",
            params![crew, principal, uuid, n],
        )?;
    }
    Ok(())
}

fn knowledge_targets(v: &serde_json::Value) -> Vec<TargetRef> {
    let mut result = Vec::new();
    match v {
        serde_json::Value::Object(m) => {
            if let Ok(t @ (TargetRef::Document { .. } | TargetRef::Knowledge { .. })) =
                serde_json::from_value(v.clone())
            {
                result.push(t)
            } else {
                for v in m.values() {
                    result.extend(knowledge_targets(v))
                }
            }
        }
        serde_json::Value::Array(a) => {
            for v in a {
                result.extend(knowledge_targets(v))
            }
        }
        _ => (),
    }
    result
}
pub fn knowledge_edges(c: &Connection, crew: &str, namespace: &str) -> Result<Vec<Edge>> {
    let mut q=c.prepare("SELECT id,owner_crew,source_json,target_json,origin,created_at FROM managed_links WHERE json_extract(target_json,'$.kind') IN('document','knowledge') AND json_extract(target_json,'$.source.context.crewId')=?1 AND json_extract(target_json,'$.source.context.namespace')=?2 ORDER BY id")?;
    q.query_map(params![crew, namespace], read)?
        .map(|r| edge(r?))
        .collect()
}
pub fn retire_knowledge(c: &Connection, target: &TargetRef) -> Result<()> {
    c.execute("UPDATE managed_v2_receipts SET retired=1,input_json=NULL,result_json=NULL WHERE (owner_crew,principal,request_id) IN(SELECT owner_crew,principal,request_id FROM managed_v2_receipt_knowledge WHERE target_key=?1)",[target_key(target)?])?;
    Ok(())
}
pub fn affected_by_crew_purge(c: &Connection, crew: &str) -> Result<Vec<String>> {
    let mut q=c.prepare("SELECT DISTINCT l.owner_crew FROM managed_links l LEFT JOIN object_refs r ON r.number=l.target_number LEFT JOIN sessions s ON s.id=l.target_session LEFT JOIN channels ch ON ch.id=l.target_channel LEFT JOIN messages m ON m.id=l.target_message LEFT JOIN channels mch ON mch.id=m.channel_id WHERE l.owner_crew!=?1 AND (l.target_crew=?1 OR r.crew_id=?1 OR s.crew_id=?1 OR ch.crew_id=?1 OR mch.crew_id=?1 OR json_extract(l.target_json,'$.source.context.crewId')=?1) ORDER BY l.owner_crew")?;
    Ok(q.query_map([crew], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Filter unreadable Ask sources in SQL before pagination; all other current
/// source kinds are organization-readable (targets are still checked above).
pub fn incoming_visible(
    c: &Connection,
    target: &TargetRef,
    after: i64,
    limit: u16,
    ask_crew: Option<&str>,
) -> Result<Vec<Edge>> {
    let mut q=c.prepare("SELECT id,owner_crew,source_json,target_json,origin,created_at FROM managed_links WHERE target_key=?1 AND id>?2 AND (?4 IS NULL OR json_extract(source_json,'$.kind')!='ask' OR owner_crew=?4) ORDER BY id LIMIT ?3")?;
    q.query_map(params![target_key(target)?, after, limit, ask_crew], read)?
        .map(|r| edge(r?))
        .collect()
}

pub fn deletions_empty(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT NOT EXISTS(SELECT 1 FROM managed_link_deleted_rows) AND NOT EXISTS(SELECT 1 FROM managed_link_deleted_sources)",[],|r|r.get(0))?)
}
/// Once per successful collaboration transaction; PK queues dedupe the whole cascade.
pub fn settle_deletions(c: &Connection) -> Result<Vec<String>> {
    managed_objects::transaction(c)?;
    let mut crews = std::collections::BTreeSet::new();
    let rows = {
        let mut q=c.prepare("SELECT v.number,v.revision,r.crew_id FROM managed_link_deleted_rows d JOIN managed_rows v ON v.number=d.number JOIN object_refs r ON r.number=v.number ORDER BY v.number")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (n, rev, crew) in rows {
        let next = rev.checked_add(1).ok_or_else(bad)?;
        c.execute(
            "UPDATE managed_rows SET revision=?2 WHERE number=?1",
            params![n, next],
        )?;
        crews.insert(crew);
    }
    let sources = {
        let mut q=c.prepare("SELECT source_key,owner_crew FROM managed_link_deleted_sources ORDER BY source_key,owner_crew")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (s, crew) in sources {
        let source: SourceRef = serde_json::from_str(&s)?;
        advance(c, &source, &crew)?;
        crews.insert(crew);
    }
    c.execute("DELETE FROM managed_link_deleted_rows", [])?;
    c.execute("DELETE FROM managed_link_deleted_sources", [])?;
    Ok(crews.into_iter().collect())
}
pub fn enqueue_row(c: &Connection, n: i64) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO managed_link_deleted_rows VALUES(?1)",
        [n],
    )?;
    Ok(())
}
pub fn enqueue_source(c: &Connection, s: &SourceRef, crew: &str) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO managed_link_deleted_sources VALUES(?1,?2)",
        params![source_key(s)?, crew],
    )?;
    Ok(())
}

pub fn adapter_sources(
    c: &Connection,
    after: i64,
    inclusive: bool,
    limit: u16,
    ask_crew: Option<&str>,
) -> Result<Vec<(i64, String)>> {
    let mut q=c.prepare("SELECT number,kind FROM object_refs WHERE (number>?1 OR (?2 AND number=?1)) AND kind IN('plan','requirement','human_task','decision','risk','lesson','table','custom_row') AND (?4 IS NULL OR kind!='human_task' OR crew_id=?4) ORDER BY number LIMIT ?3")?;
    Ok(
        q.query_map(params![after, inclusive, limit, ask_crew], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?,
    )
}

pub fn artifact_adapter_rows(
    c: &Connection,
    number: i64,
    after: i64,
    limit: u16,
) -> Result<Vec<(i64, crewd_proto::managed_artifacts::Reference, String, bool)>> {
    let mut q=c.prepare("SELECT e.id,e.reference,e.relation,NOT EXISTS(SELECT 1 FROM managed_artifact_edges p WHERE p.target_number=e.target_number AND p.reference=e.reference AND p.relation=e.relation AND p.id<e.id) FROM managed_artifact_edges e WHERE e.target_number=?1 AND e.id>?2 ORDER BY e.id LIMIT ?3")?;
    q.query_map(params![number, after, limit], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, bool>(3)?,
        ))
    })?
    .map(|r| {
        let (id, reference, relation, first) = r?;
        Ok((id, serde_json::from_str(&reference)?, relation, first))
    })
    .collect()
}
