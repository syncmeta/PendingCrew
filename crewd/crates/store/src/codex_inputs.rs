//! Private source/actor-bound native input bytes. Never a file server or task ledger.
use crate::{Result, StoreError};
use crewd_proto::codex_inputs::*;
use ring::digest::{SHA256, digest};
use rusqlite::{Connection, OptionalExtension, params};

const DDL: &str = "CREATE TABLE IF NOT EXISTS codex_native_inputs(id TEXT PRIMARY KEY,db_id TEXT NOT NULL,principal TEXT NOT NULL,crew_id TEXT NOT NULL,origin_session_id TEXT,bound_session_id TEXT,kind TEXT NOT NULL,filename TEXT NOT NULL,mime_type TEXT NOT NULL,sha256 TEXT NOT NULL,body BLOB,state TEXT NOT NULL CHECK(state IN('prepared','consumed','discarded')),consumed_request TEXT,created_at TEXT NOT NULL);CREATE TABLE IF NOT EXISTS codex_input_requests(request_id TEXT PRIMARY KEY,db_id TEXT NOT NULL,principal TEXT NOT NULL,crew_id TEXT NOT NULL,operation TEXT NOT NULL,digest TEXT NOT NULL,result TEXT NOT NULL CHECK(json_valid(result)));";
#[derive(Debug, Clone)]
pub struct Owner {
    pub db_id: String,
    pub principal: String,
}
#[derive(Clone)]
pub struct Payload {
    pub reference: CodexInputRef,
    pub filename: String,
    pub mime_type: String,
    pub body: Vec<u8>,
}
impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeInputPayload([private bytes redacted])")
    }
}
fn invalid() -> StoreError {
    StoreError::Invalid("native input scope/content/state invalid".into())
}
pub fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn exists(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='codex_native_inputs')",[],|r|r.get(0))?)
}
fn current(c: &Connection, owner: &Owner, crew: &str) -> Result<()> {
    if owner.principal.is_empty()
        || crate::meta_get(c, "dbId")?.as_deref() != Some(owner.db_id.as_str())
        || !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM crews WHERE id=?1 AND archived_at IS NULL)",
            [crew],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid());
    }
    Ok(())
}
fn transaction(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn activate(c: &Connection) -> Result<()> {
    transaction(c)?;
    if c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? != 19 {
        return Err(invalid());
    }
    c.execute_batch(DDL)?;
    Ok(())
}
fn extension(c: &Connection, session: &str) -> Result<()> {
    let mut state = crate::codex_controls::get(c, session)?;
    if !crate::codex_controls::native_ready(c, session, None)? {
        return Err(invalid());
    }
    state.version = 2;
    crate::codex_controls::put(c, session, &state)
}
pub fn lookup(
    c: &Connection,
    owner: &Owner,
    p: &CodexInputLookupParams,
) -> Result<CodexInputLookup> {
    current(c, owner, &p.crew_id)?;
    if !crewd_proto::valid_remote_uuid(&p.request_id) {
        return Err(invalid());
    }
    if !exists(c)? {
        return Ok(CodexInputLookup::NotFound);
    }
    let raw:Option<String>=c.query_row("SELECT result FROM codex_input_requests WHERE request_id=?1 AND db_id=?2 AND principal=?3 AND crew_id=?4",params![p.request_id,owner.db_id,owner.principal,p.crew_id],|r|r.get(0)).optional()?;
    Ok(match raw {
        Some(raw) => CodexInputLookup::Known {
            result: serde_json::from_str(&raw)?,
        },
        None => CodexInputLookup::NotFound,
    })
}
fn original(
    c: &Connection,
    owner: &Owner,
    crew: &str,
    request: &str,
    operation: &str,
    digest: &str,
) -> Result<Option<CodexInputResult>> {
    let row:Option<(String,String,String,String,String,String)>=c.query_row("SELECT db_id,principal,crew_id,operation,digest,result FROM codex_input_requests WHERE request_id=?1",[request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    match row {
        None => Ok(None),
        Some((db, who, scope, op, body, result)) => {
            if db != owner.db_id
                || who != owner.principal
                || scope != crew
                || op != operation
                || body != digest
            {
                return Err(invalid());
            }
            Ok(Some(serde_json::from_str(&result)?))
        }
    }
}
fn record(
    c: &Connection,
    owner: &Owner,
    crew: &str,
    request: &str,
    operation: &str,
    digest: &str,
    result: &CodexInputResult,
) -> Result<()> {
    c.execute("INSERT INTO codex_input_requests(request_id,db_id,principal,crew_id,operation,digest,result)VALUES(?1,?2,?3,?4,?5,?6,?7)",params![request,owner.db_id,owner.principal,crew,operation,digest,serde_json::to_string(result)?])?;
    Ok(())
}
fn format(kind: CodexInputKind, mime: &str, body: &[u8]) -> bool {
    match kind {
        CodexInputKind::Text => {
            mime == "text/plain"
                && body.len() <= MAX_TEXT_FILE_BYTES
                && std::str::from_utf8(body).is_ok()
                && !body.contains(&0)
        }
        CodexInputKind::Image => match mime {
            "image/png" => body.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => body.starts_with(b"\xff\xd8\xff"),
            "image/gif" => body.starts_with(b"GIF87a") || body.starts_with(b"GIF89a"),
            "image/webp" => body.starts_with(b"RIFF") && body.get(8..12) == Some(b"WEBP"),
            _ => false,
        },
        CodexInputKind::Audio => match mime {
            "audio/wav" => body.starts_with(b"RIFF") && body.get(8..12) == Some(b"WAVE"),
            "audio/mpeg" => {
                body.starts_with(b"ID3")
                    || body
                        .get(..2)
                        .is_some_and(|b| b[0] == 0xff && b[1] & 0xe0 == 0xe0)
            }
            "audio/mp4" => body.get(4..8) == Some(b"ftyp"),
            _ => false,
        },
    }
}
pub fn prepare(
    c: &Connection,
    owner: &Owner,
    p: &CodexInputPrepareParams,
    body: &[u8],
    at: &str,
) -> Result<CodexInputResult> {
    current(c, owner, &p.crew_id)?;
    transaction(c)?;
    if !crewd_proto::valid_remote_uuid(&p.request_id)
        || p.filename.trim().is_empty()
        || p.filename.len() > 128
        || p.filename
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
        || body.is_empty()
        || body.len() > MAX_MEDIA_BYTES
        || hash(body) != p.sha256
        || !format(p.kind, &p.mime_type, body)
    {
        return Err(invalid());
    }
    activate(c)?;
    let digest = hash(&serde_json::to_vec(p)?);
    if let Some(result) = original(c, owner, &p.crew_id, &p.request_id, INPUT_PREPARE, &digest)? {
        return Ok(result);
    }
    if let Some(session) = &p.session_id {
        let row = crate::sessions::get(c, session)?.ok_or_else(invalid)?;
        if row.crew_id != p.crew_id || row.executor != "codex" {
            return Err(invalid());
        }
        extension(c, session)?;
    }
    let duplicate:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM codex_native_inputs WHERE db_id=?1 AND principal=?2 AND crew_id=?3 AND origin_session_id IS ?4 AND sha256=?5 AND state!='discarded')",params![owner.db_id,owner.principal,p.crew_id,p.session_id,p.sha256],|r|r.get(0))?;
    if duplicate {
        return Err(StoreError::Invalid("original prepared/consumed input exists; query its original request, do not replace UUID".into()));
    }
    let total: i64 = c.query_row(
        "SELECT COALESCE(SUM(length(body)),0) FROM codex_native_inputs",
        [],
        |r| r.get(0),
    )?;
    if total + body.len() as i64 > MAX_CORE_INPUT_BYTES {
        return Err(StoreError::Invalid(
            "native Core input quota reached".into(),
        ));
    }
    if let Some(session) = &p.session_id {
        let total:i64=c.query_row("SELECT COALESCE(SUM(length(body)),0) FROM codex_native_inputs WHERE bound_session_id=?1",[session],|r|r.get(0))?;
        if total + body.len() as i64 > MAX_SESSION_INPUT_BYTES {
            return Err(StoreError::Invalid(
                "native session input quota reached".into(),
            ));
        }
    }
    let reference = CodexInputRef {
        input_id: uuid::Uuid::new_v4().to_string(),
        sha256: p.sha256.clone(),
        kind: p.kind,
        bytes: body.len() as u32,
    };
    c.execute("INSERT INTO codex_native_inputs(id,db_id,principal,crew_id,origin_session_id,bound_session_id,kind,filename,mime_type,sha256,body,state,created_at)VALUES(?1,?2,?3,?4,?5,?5,?6,?7,?8,?9,?10,'prepared',?11)",params![reference.input_id,owner.db_id,owner.principal,p.crew_id,p.session_id,serde_json::to_string(&p.kind)?,p.filename,p.mime_type,p.sha256,body,at])?;
    let result = CodexInputResult::Prepared { input: reference };
    record(
        c,
        owner,
        &p.crew_id,
        &p.request_id,
        INPUT_PREPARE,
        &digest,
        &result,
    )?;
    Ok(result)
}
fn payload(
    c: &Connection,
    owner: &Owner,
    crew: &str,
    reference: &CodexInputRef,
) -> Result<(Payload, String, Option<String>, Option<String>)> {
    let row:Option<(String,String,String,String,Vec<u8>,String,Option<String>,Option<String>)>=c.query_row("SELECT kind,filename,mime_type,sha256,body,state,bound_session_id,consumed_request FROM codex_native_inputs WHERE id=?1 AND db_id=?2 AND principal=?3 AND crew_id=?4 AND body IS NOT NULL",params![reference.input_id,owner.db_id,owner.principal,crew],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
    let Some((kind, filename, mime_type, sha, body, state, bound, request)) = row else {
        return Err(invalid());
    };
    let kind: CodexInputKind = serde_json::from_str(&kind)?;
    if kind != reference.kind
        || sha != reference.sha256
        || hash(&body) != sha
        || body.len() != reference.bytes as usize
    {
        return Err(invalid());
    }
    Ok((
        Payload {
            reference: reference.clone(),
            filename,
            mime_type,
            body,
        },
        state,
        bound,
        request,
    ))
}
pub fn consume(
    c: &Connection,
    owner: &Owner,
    crew: &str,
    session: &str,
    request: &str,
    selection: &CodexInputSelection,
) -> Result<()> {
    transaction(c)?;
    current(c, owner, crew)?;
    selection.validate().map_err(|_| invalid())?;
    if !crewd_proto::valid_remote_uuid(request) {
        return Err(invalid());
    }
    let row = crate::sessions::get(c, session)?.ok_or_else(invalid)?;
    if row.crew_id != crew || row.executor != "codex" {
        return Err(invalid());
    }
    extension(c, session)?;
    if !selection.inputs.is_empty() && !exists(c)? {
        return Err(invalid());
    }
    let mut text_bytes = 0;
    let before: i64 = if exists(c)? {
        c.query_row("SELECT COALESCE(SUM(length(body)),0) FROM codex_native_inputs WHERE bound_session_id=?1",[session],|r|r.get(0))?
    } else {
        0
    };
    let mut new_bytes = 0;
    for reference in &selection.inputs {
        let (payload, state, bound, consumed) = payload(c, owner, crew, reference)?;
        if state != "prepared"
            || consumed.is_some()
            || bound.as_deref().is_some_and(|s| s != session)
        {
            return Err(invalid());
        }
        if reference.kind == CodexInputKind::Text {
            text_bytes += payload.body.len()
        }
        if bound.is_none() {
            new_bytes += payload.body.len() as i64
        }
    }
    if text_bytes > 1024 * 1024 || before + new_bytes > MAX_SESSION_INPUT_BYTES {
        return Err(StoreError::Invalid(
            "native input selection/session byte bound exceeded".into(),
        ));
    }
    for reference in &selection.inputs {
        if c.execute("UPDATE codex_native_inputs SET state='consumed',bound_session_id=?1,consumed_request=?2 WHERE id=?3 AND state='prepared'",params![session,request,reference.input_id])?!=1{return Err(invalid())}
    }
    Ok(())
}
pub fn for_original(
    c: &Connection,
    owner: &Owner,
    crew: &str,
    session: &str,
    request: &str,
    selection: &CodexInputSelection,
) -> Result<Vec<Payload>> {
    current(c, owner, crew)?;
    let mut values = Vec::new();
    for reference in &selection.inputs {
        let (payload, state, bound, consumed) = payload(c, owner, crew, reference)?;
        if state != "consumed"
            || bound.as_deref() != Some(session)
            || consumed.as_deref() != Some(request)
        {
            return Err(invalid());
        }
        values.push(payload);
    }
    Ok(values)
}
pub fn discard(
    c: &Connection,
    owner: &Owner,
    p: &CodexInputDiscardParams,
) -> Result<CodexInputResult> {
    current(c, owner, &p.crew_id)?;
    transaction(c)?;
    if !crewd_proto::valid_remote_uuid(&p.request_id) || !exists(c)? {
        return Err(invalid());
    }
    let digest = hash(&serde_json::to_vec(p)?);
    if let Some(result) = original(c, owner, &p.crew_id, &p.request_id, INPUT_DISCARD, &digest)? {
        return Ok(result);
    }
    let (_, state, _, used) = payload(c, owner, &p.crew_id, &p.input)?;
    if state != "prepared" || used.is_some() {
        return Err(invalid());
    }
    c.execute("UPDATE codex_native_inputs SET body=NULL,state='discarded' WHERE id=?1 AND state='prepared'",[&p.input.input_id])?;
    let result = CodexInputResult::Discarded {
        input_id: p.input.input_id.clone(),
    };
    record(
        c,
        owner,
        &p.crew_id,
        &p.request_id,
        INPUT_DISCARD,
        &digest,
        &result,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database() -> (crate::Store, Owner) {
        let s = crate::Store::open_in_memory().unwrap();
        s.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('crew','Synthetic','t','t'),('other','Other','t','t');INSERT INTO sessions(id,crew_id,role,executor,updated_at)VALUES('session','crew','worker','codex','t'),('other-session','other','worker','codex','t');").unwrap();
        let owner = Owner {
            db_id: crate::meta_get(s.conn(), "dbId").unwrap().unwrap(),
            principal: "owned-human".into(),
        };
        (s, owner)
    }
    fn input() -> (CodexInputPrepareParams, Vec<u8>) {
        let body = b"Original UTF8 input\n".to_vec();
        (
            CodexInputPrepareParams {
                request_id: uuid::Uuid::new_v4().to_string(),
                crew_id: "crew".into(),
                session_id: Some("session".into()),
                kind: CodexInputKind::Text,
                filename: "notes.txt".into(),
                mime_type: "text/plain".into(),
                sha256: hash(&body),
                bytes_base64: "original-canonical-encoded-private-body".into(),
            },
            body,
        )
    }
    #[test]
    fn original_input_prepare_lookup_consume_and_unknown_are_one_shot() {
        let (mut s, owner) = database();
        let (p, body) = input();
        assert!(matches!(
            lookup(
                s.conn(),
                &owner,
                &CodexInputLookupParams {
                    crew_id: "crew".into(),
                    request_id: p.request_id.clone()
                }
            )
            .unwrap(),
            CodexInputLookup::NotFound
        ));
        assert!(!exists(s.conn()).unwrap());
        let result = s.with_tx(|c| prepare(c, &owner, &p, &body, "t")).unwrap();
        assert_eq!(
            s.with_tx(|c| prepare(c, &owner, &p, &body, "t")).unwrap(),
            result
        );
        let CodexInputResult::Prepared { input } = result else {
            panic!()
        };
        let select = CodexInputSelection {
            inputs: vec![input.clone()],
            skills: vec![],
        };
        let request = uuid::Uuid::new_v4().to_string();
        s.with_tx(|c| consume(c, &owner, "crew", "session", &request, &select))
            .unwrap();
        assert_eq!(
            crate::codex_controls::get(s.conn(), "session")
                .unwrap()
                .version,
            2
        );
        assert_eq!(
            for_original(s.conn(), &owner, "crew", "session", &request, &select).unwrap()[0].body,
            body
        );
        assert!(
            s.with_tx(|c| consume(
                c,
                &owner,
                "crew",
                "session",
                &uuid::Uuid::new_v4().to_string(),
                &select
            ))
            .is_err()
        );
        assert!(
            s.with_tx(|c| discard(
                c,
                &owner,
                &CodexInputDiscardParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew".into(),
                    input: input.clone()
                }
            ))
            .is_err()
        );
        let mut duplicate = p.clone();
        duplicate.request_id = uuid::Uuid::new_v4().to_string();
        assert!(
            s.with_tx(|c| prepare(c, &owner, &duplicate, &body, "t"))
                .is_err()
        );
        assert!(
            for_original(
                s.conn(),
                &owner,
                "crew",
                "session",
                &uuid::Uuid::new_v4().to_string(),
                &select
            )
            .is_err()
        );
    }
    #[test]
    fn native_inputs_reject_foreign_scope_changed_digest_and_rollback_whole_selection() {
        let (mut s, owner) = database();
        let (p, body) = input();
        let CodexInputResult::Prepared { input } =
            s.with_tx(|c| prepare(c, &owner, &p, &body, "t")).unwrap()
        else {
            panic!()
        };
        let foreign = Owner {
            principal: "other-device".into(),
            ..owner.clone()
        };
        assert!(matches!(
            lookup(
                s.conn(),
                &foreign,
                &CodexInputLookupParams {
                    crew_id: "crew".into(),
                    request_id: p.request_id.clone()
                }
            )
            .unwrap(),
            CodexInputLookup::NotFound
        ));
        let mut changed = p.clone();
        changed.filename = "changed.txt".into();
        assert!(
            s.with_tx(|c| prepare(c, &owner, &changed, &body, "t"))
                .is_err()
        );
        let mut missing = input.clone();
        missing.input_id = uuid::Uuid::new_v4().to_string();
        let selection = CodexInputSelection {
            inputs: vec![input.clone(), missing],
            skills: vec![],
        };
        assert!(
            s.with_tx(|c| consume(
                c,
                &owner,
                "crew",
                "session",
                &uuid::Uuid::new_v4().to_string(),
                &selection
            ))
            .is_err()
        );
        let selection = CodexInputSelection {
            inputs: vec![input],
            skills: vec![],
        };
        assert!(
            s.with_tx(|c| consume(
                c,
                &foreign,
                "crew",
                "session",
                &uuid::Uuid::new_v4().to_string(),
                &selection
            ))
            .is_err()
        );
        assert!(
            s.with_tx(|c| consume(
                c,
                &owner,
                "other",
                "other-session",
                &uuid::Uuid::new_v4().to_string(),
                &selection
            ))
            .is_err()
        );
        s.with_tx(|c| {
            consume(
                c,
                &owner,
                "crew",
                "session",
                &uuid::Uuid::new_v4().to_string(),
                &selection,
            )
        })
        .unwrap();
    }
    #[test]
    fn unused_discard_keeps_original_receipt_but_never_exposes_bytes() {
        let (mut s, owner) = database();
        let (p, body) = input();
        let CodexInputResult::Prepared { input } =
            s.with_tx(|c| prepare(c, &owner, &p, &body, "t")).unwrap()
        else {
            panic!()
        };
        let d = CodexInputDiscardParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            crew_id: "crew".into(),
            input,
        };
        let result = s.with_tx(|c| discard(c, &owner, &d)).unwrap();
        assert_eq!(s.with_tx(|c| discard(c, &owner, &d)).unwrap(), result);
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM codex_native_inputs WHERE body IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(matches!(
            lookup(
                s.conn(),
                &owner,
                &CodexInputLookupParams {
                    crew_id: "crew".into(),
                    request_id: p.request_id
                }
            )
            .unwrap(),
            CodexInputLookup::Known {
                result: CodexInputResult::Prepared { .. }
            }
        ));
    }
    #[test]
    fn native_history_unknown_and_delete_are_durable_fences_not_business_delete() {
        use crewd_proto::{codex::CodexHistoryAction as A, commands::CommandPhase as P};
        let (mut s, _) = database();
        let request = uuid::Uuid::new_v4().to_string();
        s.with_tx(|c| crate::codex_controls::history_intent(c, "session", &request, A::Archive))
            .unwrap();
        assert!(!crate::codex_controls::native_ready(s.conn(), "session", None).unwrap());
        crate::codex_controls::settle_history(s.conn(), "session", "not-the-request", P::Confirmed)
            .unwrap();
        assert!(!crate::codex_controls::native_ready(s.conn(), "session", None).unwrap());
        crate::codex_controls::settle_history(s.conn(), "session", &request, P::Confirmed).unwrap();
        assert!(!crate::codex_controls::native_ready(s.conn(), "session", None).unwrap());
        assert!(
            crate::codex_controls::native_ready(s.conn(), "session", Some(A::Unarchive)).unwrap()
        );
        let request = uuid::Uuid::new_v4().to_string();
        s.with_tx(|c| crate::codex_controls::history_intent(c, "session", &request, A::Delete))
            .unwrap();
        crate::codex_controls::settle_history(s.conn(), "session", &request, P::Confirmed).unwrap();
        assert!(
            !crate::codex_controls::native_ready(s.conn(), "session", Some(A::Unarchive)).unwrap()
        );
        assert!(crate::sessions::get(s.conn(), "session").unwrap().is_some());
    }
}
