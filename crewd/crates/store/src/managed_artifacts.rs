//! Unverified natural-key pointers. Only current committed facts are captured; no history scan.
use crate::{
    Result, StoreError,
    object_refs::{self, ObjectKind},
};
use crewd_proto::{managed_artifacts::*, managed_objects::Lesson};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

pub const MAX_VALUE_BYTES: usize = 4096;
pub const MAX_REPOSITORY_BYTES: usize = 512;
pub const PAGE: usize = 16;
pub const DDL: &str = r#"
CREATE TABLE managed_artifacts (
 reference TEXT PRIMARY KEY CHECK(json_valid(reference)), created_at TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE managed_artifact_claims (
 reference TEXT NOT NULL REFERENCES managed_artifacts(reference) ON DELETE CASCADE,
 crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 PRIMARY KEY(reference,crew_id)
) WITHOUT ROWID;
CREATE TABLE managed_artifact_edges (
 id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0), reference TEXT NOT NULL REFERENCES managed_artifacts(reference) ON DELETE CASCADE,
 owner_crew TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 target_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
 subject_json TEXT NOT NULL CHECK(json_valid(subject_json)), relation TEXT NOT NULL,
 origin_key TEXT NOT NULL, origin_json TEXT NOT NULL CHECK(json_valid(origin_json)), created_at TEXT NOT NULL,
 UNIQUE(reference,owner_crew,target_number,relation,origin_key)
);
CREATE INDEX managed_artifact_backlinks ON managed_artifact_edges(reference,id);
CREATE INDEX managed_artifact_subject ON managed_artifact_edges(target_number,id);
CREATE TABLE managed_artifact_receipts (
 crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE, principal TEXT NOT NULL, request_id TEXT NOT NULL,
 digest BLOB NOT NULL CHECK(length(digest)=32), result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
 target_number INTEGER, retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
 PRIMARY KEY(crew_id,principal,request_id), CHECK((retired=0 AND result_json IS NOT NULL) OR (retired=1 AND result_json IS NULL))
) WITHOUT ROWID;
CREATE INDEX managed_artifact_receipt_target ON managed_artifact_receipts(target_number);
CREATE TRIGGER managed_artifact_retire_target BEFORE DELETE ON object_refs BEGIN
 UPDATE managed_artifact_receipts SET result_json=NULL,retired=1 WHERE target_number=OLD.number;
END;
CREATE TRIGGER managed_artifact_edge_gc AFTER DELETE ON managed_artifact_edges BEGIN
 DELETE FROM managed_artifacts WHERE reference=OLD.reference
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_edges WHERE reference=OLD.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_claims WHERE reference=OLD.reference);
END;
CREATE TRIGGER managed_artifact_claim_gc AFTER DELETE ON managed_artifact_claims BEGIN
 DELETE FROM managed_artifacts WHERE reference=OLD.reference
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_edges WHERE reference=OLD.reference)
 AND NOT EXISTS(SELECT 1 FROM managed_artifact_claims WHERE reference=OLD.reference);
END;
"#;
fn bad() -> StoreError {
    StoreError::Invalid("invalid artifact identity, relation or receipt".into())
}
fn transaction(c: &Connection) -> Result<()> {
    crate::managed_objects::transaction(c)
}
fn text(v: &str, max: usize) -> bool {
    !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control)
}
fn repository(v: &str) -> bool {
    text(v, MAX_REPOSITORY_BYTES) && v.trim() == v && !v.trim().is_empty()
}

/// URL parsing is structural only. Scheme/host/port are canonical; the original
/// path/query/fragment bytes are preserved (an empty path is represented by `/`).
fn url(v: &str) -> Option<String> {
    if !text(v, MAX_VALUE_BYTES) || v.chars().any(char::is_whitespace) || v.contains('\\') {
        return None;
    }
    let (_, rest) = v.split_once("://")?;
    let cut = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, suffix) = rest.split_at(cut);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let parsed = url::Url::parse(v).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let host = match parsed.host()? {
        url::Host::Domain(host) => host.to_ascii_lowercase(),
        url::Host::Ipv4(host) => host.to_string(),
        url::Host::Ipv6(host) => format!("[{host}]"),
    };
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let bytes = suffix.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'%'
            && (i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit())
        {
            return None;
        }
    }
    let slash = if suffix.starts_with('/') { "" } else { "/" };
    Some(format!("{}://{host}{port}{slash}{suffix}", parsed.scheme()))
}

pub fn canonicalize(crew: &str, input: &Input) -> Result<Reference> {
    if !text(crew, 512) || input.repository.as_deref().is_some_and(|v| !repository(v)) {
        return Err(bad());
    }
    if input.value.len() > MAX_VALUE_BYTES
        || input.value.chars().any(char::is_control)
        || (matches!(input.kind, Kind::Link | Kind::PullRequest)
            && input.value.chars().any(char::is_whitespace))
    {
        return Err(bad());
    }
    let value = input.value.trim();
    if !text(value, MAX_VALUE_BYTES) {
        return Err(bad());
    }
    let scoped = || {
        input.repository.as_ref().map_or_else(
            || Namespace::Unassigned {
                crew_id: crew.into(),
            },
            |id| Namespace::Repository { id: id.clone() },
        )
    };
    let (namespace, key) = match input.kind {
        Kind::Commit => {
            let v = value.strip_prefix("commit:").unwrap_or(value);
            if !(7..=64).contains(&v.len()) || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
            (
                if matches!(v.len(), 40 | 64) {
                    Namespace::Global
                } else {
                    scoped()
                },
                format!("commit:{}", v.to_ascii_lowercase()),
            )
        }
        Kind::PullRequest | Kind::Link => {
            let v = value
                .strip_prefix("pr:")
                .or_else(|| value.strip_prefix("url:"))
                .unwrap_or(value);
            (
                Namespace::Global,
                format!("url:{}", url(v).ok_or_else(bad)?),
            )
        }
        Kind::File => {
            let v = value.strip_prefix("file:").unwrap_or(value);
            if v.is_empty()
                || v.starts_with('/')
                || v.contains('\\')
                || v.as_bytes().get(1) == Some(&b':')
                || v.split('/').any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(bad());
            }
            (scoped(), format!("file:{v}"))
        }
        Kind::Test => {
            let v = value.strip_prefix("test:").unwrap_or(value);
            if v.trim().is_empty() {
                return Err(bad());
            }
            (scoped(), format!("test:{v}"))
        }
    };
    Ok(Reference { namespace, key })
}
fn key(reference: &Reference) -> Result<String> {
    let (kind, value) = reference.key.split_once(':').ok_or_else(bad)?;
    let kind = match kind {
        "commit" => Kind::Commit,
        "url" => Kind::Link,
        "file" => Kind::File,
        "test" => Kind::Test,
        _ => return Err(bad()),
    };
    let (crew, repository) = match &reference.namespace {
        Namespace::Global => ("validation", None),
        Namespace::Repository { id } => ("validation", Some(id.clone())),
        Namespace::Unassigned { crew_id } => (crew_id.as_str(), None),
    };
    if canonicalize(
        crew,
        &Input {
            kind,
            value: value.into(),
            repository,
        },
    )? != *reference
    {
        return Err(bad());
    }
    Ok(serde_json::to_string(reference)?)
}
pub fn get(c: &Connection, reference: &Reference) -> Result<Option<Artifact>> {
    let key = key(reference)?;
    Ok(c.query_row(
        "SELECT created_at FROM managed_artifacts WHERE reference=?1",
        [key],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(|created_at| Artifact {
        reference: reference.clone(),
        created_at,
        verification: Verification::Unverified,
    }))
}
pub fn ensure(c: &Connection, reference: &Reference, now: &str) -> Result<(Artifact, bool)> {
    transaction(c)?;
    let changed = c.execute(
        "INSERT OR IGNORE INTO managed_artifacts(reference,created_at) VALUES(?1,?2)",
        params![key(reference)?, now],
    )?;
    Ok((get(c, reference)?.ok_or_else(bad)?, changed == 1))
}
pub fn claim(c: &Connection, crew: &str, reference: &Reference) -> Result<bool> {
    transaction(c)?;
    Ok(c.execute(
        "INSERT OR IGNORE INTO managed_artifact_claims(reference,crew_id) VALUES(?1,?2)",
        params![key(reference)?, crew],
    )? == 1)
}
pub fn list(c: &Connection, after: Option<&Reference>) -> Result<Page> {
    let after = after.map(key).transpose()?.unwrap_or_default();
    let mut q=c.prepare("SELECT reference,created_at FROM managed_artifacts WHERE reference>?1 ORDER BY reference LIMIT 17")?;
    let mut items = q
        .query_map([after], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .map(|r| {
            let (v, created_at) = r?;
            Ok(Artifact {
                reference: serde_json::from_str(&v)?,
                created_at,
                verification: Verification::Unverified,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let next_after = (items.len() > PAGE).then(|| items[PAGE - 1].reference.clone());
    items.truncate(PAGE);
    Ok(Page { items, next_after })
}
fn positive(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0 && number.to_string() == value)
        .ok_or_else(bad)
}
fn subject_identity(subject: &Subject) -> Result<(ObjectKind, i64)> {
    let (id, prefix, kind) = match subject {
        Subject::Work { id } => (id, "W", ObjectKind::Plan),
        Subject::Requirement { id } => (id, "#", ObjectKind::Requirement),
        Subject::Decision { id } => (id, "D", ObjectKind::Decision),
        Subject::Risk { id } => (id, "R", ObjectKind::Risk),
        Subject::Lesson { id } => (id, "L", ObjectKind::Lesson),
        Subject::Row { id } => (id, "O", ObjectKind::CustomRow),
        Subject::Table { id } => (id, "T", ObjectKind::Table),
    };
    let value = id.strip_prefix(prefix).ok_or_else(bad)?;
    let number = value
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or_else(bad)?;
    Ok((kind, number))
}
pub fn subject_number(c: &Connection, subject: &Subject) -> Result<i64> {
    let (kind, number) = subject_identity(subject)?;
    object_refs::get(c, kind, number)?.ok_or_else(bad)?;
    Ok(number)
}

pub fn relation_valid(subject: &Subject, relation: Relation, reference: &Reference) -> bool {
    match relation {
        Relation::Relates => true,
        Relation::Evidence => matches!(
            subject,
            Subject::Work { .. } | Subject::Requirement { .. } | Subject::Lesson { .. }
        ),
        Relation::AppliesTo => {
            matches!(subject, Subject::Lesson { .. }) && reference.key.starts_with("file:")
        }
    }
}
fn relation(v: Relation) -> &'static str {
    match v {
        Relation::Evidence => "evidence",
        Relation::AppliesTo => "appliesTo",
        Relation::Relates => "relates",
    }
}
fn origin(v: &Origin) -> String {
    match v {
        Origin::Manual { .. } => "manual".into(),
        Origin::WorkDone { .. } => "work".into(),
        Origin::LessonEvidence => "lesson".into(),
    }
}
fn edge_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, String, String, String)> {
    Ok((
        r.get::<_, i64>(0)?.to_string(),
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}
fn decode_edge(
    (id, reference, crew_id, subject, rel, origin, created_at): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ),
) -> Result<Edge> {
    Ok(Edge {
        id,
        reference: serde_json::from_str(&reference)?,
        crew_id,
        subject: serde_json::from_str(&subject)?,
        relation: serde_json::from_str(&format!("\"{rel}\""))?,
        origin: serde_json::from_str(&origin)?,
        created_at,
    })
}
pub fn link(
    c: &Connection,
    crew: &str,
    reference: &Reference,
    subject: &Subject,
    rel: Relation,
    provenance: &Origin,
    now: &str,
) -> Result<(Edge, bool)> {
    transaction(c)?;
    if !relation_valid(subject, rel, reference) {
        return Err(bad());
    }
    let n = subject_number(c, subject)?;
    let k = key(reference)?;
    let origin_key = origin(provenance);
    let existing=c.query_row("SELECT id,reference,owner_crew,subject_json,relation,origin_json,created_at FROM managed_artifact_edges WHERE reference=?1 AND owner_crew=?2 AND target_number=?3 AND relation=?4 AND origin_key=?5",params![k,crew,n,relation(rel),origin_key],edge_row).optional()?;
    if let Some(edge) = existing {
        return Ok((decode_edge(edge)?, false));
    }
    c.execute("INSERT INTO managed_artifact_edges(reference,owner_crew,target_number,subject_json,relation,origin_key,origin_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![k,crew,n,serde_json::to_string(subject)?,relation(rel),origin_key,serde_json::to_string(provenance)?,now])?;
    let edge=c.query_row("SELECT id,reference,owner_crew,subject_json,relation,origin_json,created_at FROM managed_artifact_edges WHERE id=?1",[c.last_insert_rowid()],edge_row)?;
    Ok((decode_edge(edge)?, true))
}
pub fn unlink(
    c: &Connection,
    crew: &str,
    reference: &Reference,
    subject: &Subject,
    rel: Relation,
    id: &str,
) -> Result<()> {
    transaction(c)?;
    let id = positive(id)?;
    let count=c.execute("DELETE FROM managed_artifact_edges WHERE id=?1 AND reference=?2 AND owner_crew=?3 AND target_number=?4 AND relation=?5 AND origin_key='manual'",params![id,key(reference)?,crew,subject_number(c,subject)?,relation(rel)])?;
    if count != 1 {
        return Err(bad());
    }
    Ok(())
}
pub fn edges(
    c: &Connection,
    reference: Option<&Reference>,
    subject: Option<&Subject>,
    after: Option<&str>,
) -> Result<EdgePage> {
    let after = after.map(positive).transpose()?.unwrap_or(0);
    let k = reference.map(key).transpose()?;
    let n = subject.map(|v| subject_number(c, v)).transpose()?;
    let mut q=c.prepare("SELECT id,reference,owner_crew,subject_json,relation,origin_json,created_at FROM managed_artifact_edges WHERE (?1 IS NULL OR reference=?1) AND (?2 IS NULL OR target_number=?2) AND id>?3 ORDER BY id LIMIT 17")?;
    let mut items = q
        .query_map(params![k, n, after], edge_row)?
        .map(|r| decode_edge(r?))
        .collect::<Result<Vec<_>>>()?;
    let next_after = (items.len() > PAGE).then(|| items[PAGE - 1].id.clone());
    items.truncate(PAGE);
    Ok(EdgePage { items, next_after })
}
pub fn gc(c: &Connection) -> Result<()> {
    transaction(c)?;
    c.execute("DELETE FROM managed_artifacts WHERE NOT EXISTS(SELECT 1 FROM managed_artifact_claims c WHERE c.reference=managed_artifacts.reference) AND NOT EXISTS(SELECT 1 FROM managed_artifact_edges e WHERE e.reference=managed_artifacts.reference)",[])?;
    Ok(())
}
pub enum Saved {
    Active { digest: [u8; 32], receipt: Receipt },
    Retired,
}
fn manual_actor_valid(origin: &Origin) -> bool {
    match origin {
        Origin::Manual { actor } if actor.kind == "human" => actor.session_id.is_none(),
        Origin::Manual { actor } if actor.kind == "session" => {
            actor.session_id.as_deref().is_some_and(|id| text(id, 512))
        }
        _ => false,
    }
}
/// Validate only original immutable facts. Current anchors/edges may legitimately
/// have been collected after unlink; a historical receipt must not be reprojected.
fn receipt_valid(receipt: &Receipt, target: Option<i64>, digest: &[u8; 32]) -> Result<()> {
    if !text(&receipt.crew_id, 512)
        || !crewd_proto::valid_remote_uuid(&receipt.request_id)
        || !text(&receipt.scope.db_id, 256)
        || !crewd_proto::valid_remote_uuid(&receipt.scope.restore_epoch)
        || receipt.recorded_at.trim().is_empty()
        || receipt.artifact.created_at.trim().is_empty()
        || receipt.recorded_at.chars().any(char::is_control)
        || receipt.artifact.created_at.chars().any(char::is_control)
    {
        return Err(bad());
    }
    key(&receipt.artifact.reference)?;
    let original = Submit {
        scope: receipt.scope.clone(),
        crew_id: receipt.crew_id.clone(),
        request_id: receipt.request_id.clone(),
        change: receipt.change.clone(),
    };
    if ring::digest::digest(&ring::digest::SHA256, &serde_json::to_vec(&original)?).as_ref()
        != digest.as_slice()
    {
        return Err(bad());
    }
    let validate_link = |reference: &Reference,
                         subject: &Subject,
                         rel: Relation,
                         created: bool,
                         added: bool,
                         edge: &Edge|
     -> Result<()> {
        if reference != &receipt.artifact.reference
            || &edge.reference != reference
            || edge.crew_id != receipt.crew_id
            || &edge.subject != subject
            || edge.relation != rel
            || !manual_actor_valid(&edge.origin)
            || edge.created_at.trim().is_empty()
            || edge.created_at.chars().any(char::is_control)
            || !relation_valid(subject, rel, reference)
            || (created && !added)
            || (added && edge.created_at != receipt.recorded_at)
            || (created && receipt.artifact.created_at != receipt.recorded_at)
            || target != Some(subject_identity(subject)?.1)
        {
            return Err(bad());
        }
        positive(&edge.id)?;
        Ok(())
    };
    match (&receipt.change, &receipt.effect) {
        (
            Change::Register { input },
            Effect::Registered {
                created,
                claim_added,
            },
        ) => {
            if target.is_some()
                || canonicalize(&receipt.crew_id, input)? != receipt.artifact.reference
                || (*created && !*claim_added)
                || (*created && receipt.artifact.created_at != receipt.recorded_at)
            {
                return Err(bad());
            }
        }
        (
            Change::Link {
                input,
                subject,
                relation,
            },
            Effect::Linked {
                created,
                added,
                edge,
            },
        ) => {
            validate_link(
                &canonicalize(&receipt.crew_id, input)?,
                subject,
                *relation,
                *created,
                *added,
                edge,
            )?;
        }
        (
            Change::LinkExisting {
                reference,
                subject,
                relation,
            },
            Effect::Linked {
                created,
                added,
                edge,
            },
        ) => {
            if *created {
                return Err(bad());
            }
            validate_link(reference, subject, *relation, false, *added, edge)?;
        }
        (
            Change::Unlink {
                reference,
                subject,
                relation,
                expected_edge_id,
            },
            Effect::Unlinked { edge_id },
        ) => {
            if reference != &receipt.artifact.reference
                || edge_id != expected_edge_id
                || target != Some(subject_identity(subject)?.1)
                || !relation_valid(subject, *relation, reference)
            {
                return Err(bad());
            }
            positive(edge_id)?;
        }
        _ => return Err(bad()),
    }
    Ok(())
}
pub fn receipt(
    c: &Connection,
    crew: &str,
    principal: &str,
    request: &str,
) -> Result<Option<Saved>> {
    let row=c.query_row("SELECT digest,result_json,retired,target_number FROM managed_artifact_receipts WHERE crew_id=?1 AND principal=?2 AND request_id=?3",params![crew,principal,request],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,bool>(2)?,r.get::<_,Option<i64>>(3)?))).optional()?;
    row.map(|(digest, json, retired, target)| {
        if retired {
            return Ok(Saved::Retired);
        }
        let digest = digest.try_into().map_err(|_| bad())?;
        let receipt: Receipt = serde_json::from_str(&json.ok_or_else(bad)?)?;
        if receipt.crew_id != crew || receipt.request_id != request {
            return Err(bad());
        }
        receipt_valid(&receipt, target, &digest)?;
        Ok(Saved::Active { digest, receipt })
    })
    .transpose()
}
pub fn record_receipt(
    c: &Connection,
    principal: &str,
    digest: &[u8; 32],
    receipt: &Receipt,
    target: Option<i64>,
) -> Result<()> {
    transaction(c)?;
    receipt_valid(receipt, target, digest)?;
    c.execute("INSERT INTO managed_artifact_receipts(crew_id,principal,request_id,digest,result_json,target_number) VALUES(?1,?2,?3,?4,?5,?6)",params![receipt.crew_id,principal,receipt.request_id,digest.as_slice(),serde_json::to_string(receipt)?,target])?;
    Ok(())
}
fn enabled(c: &Connection) -> Result<bool> {
    if c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? < 28 {
        return Ok(false);
    }
    // A schema28 store must have this authority even when a particular fact has no pointer.
    c.prepare("SELECT reference FROM managed_artifacts LIMIT 1")?;
    Ok(true)
}
/// No token-count truncation. Each token has the same declared 4096-byte identity bound as explicit input.
/// Raw evidence is never rewritten; unrecognized tokens are simply not artifact pointers.
fn token_reference(crew: &str, token: &str) -> Option<Reference> {
    let kind = if token.starts_with("commit:")
        || ((7..=64).contains(&token.len()) && token.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        Kind::Commit
    } else if token.starts_with("file:") {
        Kind::File
    } else if token.starts_with("test:") {
        Kind::Test
    } else if token.starts_with("pr:")
        || token.starts_with("url:")
        || token
            .get(..7)
            .is_some_and(|v| v.eq_ignore_ascii_case("http://"))
        || token
            .get(..8)
            .is_some_and(|v| v.eq_ignore_ascii_case("https://"))
    {
        Kind::Link
    } else {
        return None;
    };
    canonicalize(
        crew,
        &Input {
            kind,
            value: token.into(),
            repository: None,
        },
    )
    .ok()
}
pub fn extract(crew: &str, evidence: &str) -> Vec<Reference> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for raw in evidence.split_whitespace() {
        let wrapped = raw.trim_matches(|c: char| {
            matches!(
                c,
                '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '"' | '\'' | '`'
            )
        });
        // Preserve a complete valid token first, including IPv6 brackets and URI punctuation.
        if let Some(reference) = [raw, wrapped]
            .into_iter()
            .find_map(|token| token_reference(crew, token))
        {
            if seen.insert(reference.clone()) {
                result.push(reference);
            }
            continue;
        }
        // Recognize explicit HTTP destinations inside Markdown or prose prefixes.
        // Parentheses inside the URL are balanced; an enclosing Markdown ')' is excluded.
        let mut consumed = 0;
        for (start, _) in raw.char_indices() {
            if start < consumed {
                continue;
            }
            let tail = &raw[start..];
            if !tail
                .get(..7)
                .is_some_and(|v| v.eq_ignore_ascii_case("http://"))
                && !tail
                    .get(..8)
                    .is_some_and(|v| v.eq_ignore_ascii_case("https://"))
            {
                continue;
            }
            let mut depth = 0usize;
            let mut end = tail.len();
            for (i, ch) in tail.char_indices() {
                if i > MAX_VALUE_BYTES {
                    end = i;
                    break;
                }
                if matches!(
                    ch,
                    '"' | '\''
                        | '`'
                        | '<'
                        | '>'
                        | '\u{3002}'
                        | '\u{ff0c}'
                        | '\u{ff1b}'
                        | '\u{ff09}'
                        | '\u{3011}'
                        | '\u{300d}'
                ) || (ch == ')' && depth == 0)
                {
                    end = i;
                    break;
                }
                if ch == '(' {
                    depth += 1;
                } else if ch == ')' {
                    depth -= 1;
                }
            }
            consumed = start + end;
            if let Some(reference) = token_reference(crew, &tail[..end]) {
                if seen.insert(reference.clone()) {
                    result.push(reference);
                }
            }
        }
    }
    result
}

pub fn capture_done(
    c: &Connection,
    crew: &str,
    event: &crewd_work::Event,
    now: &str,
) -> Result<()> {
    if !enabled(c)? {
        return Ok(());
    }
    transaction(c)?;
    let crewd_work::EventKind::Done {
        id,
        evidence: Some(evidence),
        ..
    } = &event.kind
    else {
        return Ok(());
    };
    let subject_ref =
        crate::work_organization::reference_for_core(c, ObjectKind::Plan, &id.to_string())?
            .ok_or_else(bad)?;
    if subject_ref.crew_id != crew {
        return Err(bad());
    }
    let stored: Option<String> = c
        .query_row(
            "SELECT body FROM organization_work_events WHERE crew_id=?1 AND seq=?2",
            params![crew, i64::try_from(event.seq).map_err(|_| bad())?],
            |r| r.get(0),
        )
        .optional()?;
    if stored.as_deref() != Some(serde_json::to_string(event)?.as_str()) {
        return Err(bad());
    }
    let subject = Subject::Work {
        id: subject_ref.id(),
    };
    let value = match evidence {
        crewd_work::Evidence::Commit(v)
        | crewd_work::Evidence::Url(v)
        | crewd_work::Evidence::Text(v) => v,
    };
    let mut pointers = extract(crew, value);
    // Typed short commits need no prose prefix; malformed old evidence stays untouched.
    if let crewd_work::Evidence::Commit(v) = evidence
        && let Ok(reference) = canonicalize(
            crew,
            &Input {
                kind: Kind::Commit,
                value: v.clone(),
                repository: None,
            },
        )
        && !pointers.contains(&reference)
    {
        pointers.push(reference);
    }
    for reference in pointers {
        ensure(c, &reference, now)?;
        link(
            c,
            crew,
            &reference,
            &subject,
            Relation::Evidence,
            &Origin::WorkDone {
                event_seq: event.seq.to_string(),
            },
            now,
        )?;
    }
    Ok(())
}
pub fn capture_lesson(c: &Connection, lesson: &Lesson, now: &str) -> Result<()> {
    if !enabled(c)? {
        return Ok(());
    }
    transaction(c)?;
    let n = subject_number(
        c,
        &Subject::Lesson {
            id: lesson.id.clone(),
        },
    )?;
    if crate::managed_risks_lessons::lesson(c, n)?.as_ref() != Some(lesson) {
        return Err(bad());
    }
    let subject = Subject::Lesson {
        id: lesson.id.clone(),
    };
    for raw in &lesson.draft.evidence {
        for reference in extract(&lesson.crew_id, raw) {
            ensure(c, &reference, now)?;
            link(
                c,
                &lesson.crew_id,
                &reference,
                &subject,
                Relation::Evidence,
                &Origin::LessonEvidence,
                now,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre28_fact_hook_is_inert_and_schema28_missing_authority_refuses() {
        use crewd_work::{Actor, Config, CrewId, Evidence, NewItem, Ts, WorkBook};
        let mut book = WorkBook::new(CrewId::new("synthetic"), Config::default());
        let id = book
            .create_item(
                &Actor::worker("w"),
                Ts(1),
                NewItem::titled("Synthetic historical work").owned_by("w"),
            )
            .unwrap()
            .value;
        let done = book
            .mark_done(
                &Actor::worker("w"),
                Ts(2),
                id,
                Evidence::Commit("a".repeat(40)),
            )
            .unwrap()
            .events;
        let event = done.first().unwrap();
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA user_version=27; BEGIN").unwrap();
        capture_done(&c, "synthetic", event, "original").unwrap();
        let count: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "compatibility hook never invents authority or scans historical facts"
        );
        c.execute_batch("COMMIT; PRAGMA user_version=28; BEGIN")
            .unwrap();
        assert!(
            capture_done(&c, "synthetic", event, "current").is_err(),
            "new schema cannot silently downgrade when its tables are absent"
        );
        c.execute_batch("ROLLBACK").unwrap();
    }
}
