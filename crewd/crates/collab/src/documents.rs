//! Mutable document authority with immutable create-request witnesses.
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Sender, Tx, resource_acl as acl};
use crewd_proto::*;
use crewd_store::{StoreError, documents as db};
use rusqlite::Connection;
fn missing(id: &str) -> CollabError {
    CollabError::NotFound(Refusal::new(Code::ResourceNotFound).arg("id", id))
}
fn metadata(conn: &Connection, by: &Sender, id: &str, write: bool) -> Result<DocumentSummary> {
    let id = acl::uuid(id)?;
    let m = db::get(conn, &id)?.ok_or_else(|| missing(&id))?;
    acl::actor(conn, by, &m.crew_id, write)?;
    Ok(m)
}
fn text(title: &str, body: &str) -> Result<()> {
    if title.trim().is_empty() || title.len() > 512 {
        return Err(acl::invalid("title"));
    }
    if body.len() > 1048576 {
        return Err(acl::invalid("body"));
    }
    Ok(())
}
fn revision(m: &DocumentSummary, expected: &str) -> Result<()> {
    if expected
        .parse::<i64>()
        .ok()
        .is_none_or(|n| n <= 0 || n.to_string() != expected)
    {
        return Err(acl::invalid("expectedRevision"));
    }
    if m.revision != expected {
        let mut r = Refusal::new(Code::DocumentRevisionConflict).arg("id", &m.id);
        r.current_document = Some(Box::new(m.clone()));
        return Err(CollabError::Conflict(r));
    }
    Ok(())
}
fn changed(tx: &mut Tx<'_>, m: &DocumentSummary) {
    tx.note(Change::DocumentChanged {
        crew_id: m.crew_id.clone(),
        id: m.id.clone(),
        revision: m.revision.clone(),
        deleted: m.deleted_at.is_some(),
    });
}
pub fn create(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: DocumentCreateParams,
) -> Result<DocumentCreateResult> {
    let request = acl::uuid(&p.request_id)?;
    acl::actor(tx.conn(), by, &p.crew_id, false)?;
    let digest = acl::digest(&(&p.crew_id, &p.title, &p.body))?;
    if let Some((id, old)) = db::request(tx.conn(), &request)? {
        if old != digest {
            return Err(CollabError::Conflict(
                Refusal::new(Code::ResourceRequestConflict).arg("request", &request),
            ));
        }
        let m = metadata(tx.conn(), by, &id, false)?;
        if m.crew_id != p.crew_id {
            return Err(
                StoreError::Invalid("document request crew witness disagrees".into()).into(),
            );
        }
        return Ok(DocumentCreateResult {
            document: m,
            request_id: request,
            reused: true,
        });
    }
    text(&p.title, &p.body)?;
    let a = acl::actor(tx.conn(), by, &p.crew_id, true)?;
    let now = tx.now_ts();
    let m = DocumentSummary {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: p.crew_id,
        title: p.title,
        revision: "1".into(),
        body_length: p.body.len() as u64,
        created_by: a.clone(),
        updated_by: a,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
    };
    db::insert(tx.conn(), &m, &p.body, &request, &digest)?;
    changed(tx, &m);
    Ok(DocumentCreateResult {
        document: m,
        request_id: request,
        reused: false,
    })
}
pub fn get(conn: &Connection, by: &Sender, p: DocumentGetParams) -> Result<DocumentDetail> {
    let m = metadata(conn, by, &p.id, false)?;
    let body = db::body(conn, &m.id)?
        .ok_or_else(|| StoreError::Invalid("document metadata without body".into()))?;
    if body.len() as u64 != m.body_length {
        return Err(StoreError::Invalid("document length disagrees".into()).into());
    }
    Ok(DocumentDetail { document: m, body })
}
pub fn list(conn: &Connection, by: &Sender, p: DocumentListParams) -> Result<DocumentPage> {
    acl::actor(conn, by, &p.crew_id, false)?;
    let limit = p.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(acl::invalid("limit"));
    }
    let after = p.after.as_deref().map(acl::uuid).transpose()?;
    let mut items = db::list(
        conn,
        &p.crew_id,
        after.as_deref(),
        limit + 1,
        p.include_deleted,
    )?;
    let next = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|m| m.id.clone())
    } else {
        None
    };
    Ok(DocumentPage { items, next })
}
pub fn update(tx: &mut Tx<'_>, by: &Sender, p: DocumentUpdateParams) -> Result<DocumentSummary> {
    let m = metadata(tx.conn(), by, &p.id, true)?;
    revision(&m, &p.expected_revision)?;
    if m.deleted_at.is_some() {
        return Err(CollabError::Conflict(
            Refusal::new(Code::DocumentDeleted).arg("id", &m.id),
        ));
    }
    text(&p.title, &p.body)?;
    let a = acl::actor(tx.conn(), by, &m.crew_id, true)?;
    if !db::update(tx.conn(), &m, &p.title, &p.body, &a, &tx.now_ts())? {
        return Err(
            StoreError::Invalid("document changed inside owning transaction".into()).into(),
        );
    }
    let out = metadata(tx.conn(), by, &m.id, false)?;
    changed(tx, &out);
    Ok(out)
}
pub fn delete(tx: &mut Tx<'_>, by: &Sender, p: DocumentDeleteParams) -> Result<DocumentSummary> {
    let m = metadata(tx.conn(), by, &p.id, true)?;
    revision(&m, &p.expected_revision)?;
    if m.deleted_at.is_some() {
        return Ok(m);
    }
    let a = acl::actor(tx.conn(), by, &m.crew_id, true)?;
    if !db::delete(tx.conn(), &m, &a, &tx.now_ts())? {
        return Err(
            StoreError::Invalid("document changed inside owning transaction".into()).into(),
        );
    }
    let out = metadata(tx.conn(), by, &m.id, false)?;
    changed(tx, &out);
    Ok(out)
}

/// Bounded agent patches change exact spans of the observed revision, not an LLM reconstruction.
pub fn patch(tx: &mut Tx<'_>, by: &Sender, p: DocumentPatchParams) -> Result<DocumentSummary> {
    let m = metadata(tx.conn(), by, &p.id, true)?;
    revision(&m, &p.expected_revision)?;
    if p.edits.is_empty()
        || p.edits.len() > 20
        || p.edits.iter().map(|e| e.text.len()).sum::<usize>() > 16384
    {
        return Err(acl::invalid("edits"));
    }
    let mut body = db::body(tx.conn(), &m.id)?
        .ok_or_else(|| StoreError::Invalid("document metadata without body".into()))?;
    let mut edits = p.edits;
    edits.sort_by_key(|e| (e.start, e.end));
    let mut prior_end = 0;
    for e in &edits {
        let start = e.start as usize;
        let end = e.end as usize;
        if start < prior_end
            || start > end
            || end > body.len()
            || !body.is_char_boundary(start)
            || !body.is_char_boundary(end)
        {
            return Err(acl::invalid("edits"));
        }
        prior_end = end;
    }
    for e in edits.into_iter().rev() {
        body.replace_range(e.start as usize..e.end as usize, &e.text);
    }
    update(
        tx,
        by,
        DocumentUpdateParams {
            id: m.id,
            expected_revision: p.expected_revision,
            title: m.title,
            body,
        },
    )
}
