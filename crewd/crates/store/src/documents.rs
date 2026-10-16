//! Persistent document facts; authorization and CAS decisions belong to the owning collab Tx.
use crate::{Result, StoreError};
use crewd_proto::{DocumentSummary, ResourceActor};
use rusqlite::{Connection, OptionalExtension, params};
const META: &str =
    "id,crew_id,title,revision,body_length,created_by,updated_by,created_at,updated_at,deleted_at";
fn legacy_only(c: &Connection) -> Result<()> {
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version >= 16 || crate::meta_get(c, "knowledge.sqlite.v1")?.as_deref() == Some("1") {
        return Err(StoreError::Invalid(
            "documents use the SQLite knowledge API".into(),
        ));
    }
    Ok(())
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentSummary> {
    let actor = |i| -> rusqlite::Result<ResourceActor> {
        let raw: String = r.get(i)?;
        let a: ResourceActor = serde_json::from_str(&raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
        })?;
        if !matches!(a.kind.as_str(), "human" | "session")
            || ((a.kind == "session") != a.session_id.is_some())
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(a)
    };
    let revision: i64 = r.get(3)?;
    let len: i64 = r.get(4)?;
    if revision <= 0 || !(0..=1048576).contains(&len) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(DocumentSummary {
        id: r.get(0)?,
        crew_id: r.get(1)?,
        title: r.get(2)?,
        revision: revision.to_string(),
        body_length: len as u64,
        created_by: actor(5)?,
        updated_by: actor(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        deleted_at: r.get(9)?,
    })
}
pub fn get(conn: &Connection, id: &str) -> Result<Option<DocumentSummary>> {
    legacy_only(conn)?;
    Ok(conn
        .query_row(
            &format!("SELECT {META} FROM documents WHERE id=?1"),
            [id],
            row,
        )
        .optional()?)
}
pub fn body(conn: &Connection, id: &str) -> Result<Option<String>> {
    legacy_only(conn)?;
    Ok(conn
        .query_row("SELECT body FROM documents WHERE id=?1", [id], |r| r.get(0))
        .optional()?)
}
pub fn list(
    conn: &Connection,
    crew: &str,
    after: Option<&str>,
    limit: u32,
    deleted: bool,
) -> Result<Vec<DocumentSummary>> {
    legacy_only(conn)?;
    let mut q=conn.prepare(&format!("SELECT {META} FROM documents WHERE crew_id=?1 AND (?2 IS NULL OR id>?2) AND (?4 OR deleted_at IS NULL) ORDER BY id LIMIT ?3"))?;
    Ok(q.query_map(params![crew, after, limit, deleted], row)?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn request(conn: &Connection, id: &str) -> Result<Option<(String, Vec<u8>)>> {
    legacy_only(conn)?;
    let v: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT document_id,payload_digest FROM document_requests WHERE request_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if v.as_ref().is_some_and(|(_, d)| d.len() != 32) {
        return Err(StoreError::Invalid(
            "invalid document request witness".into(),
        ));
    }
    Ok(v)
}
pub fn insert(
    conn: &Connection,
    meta: &DocumentSummary,
    body: &str,
    request: &str,
    digest: &[u8],
) -> Result<()> {
    legacy_only(conn)?;
    conn.execute("INSERT INTO documents(id,crew_id,title,body,body_length,revision,created_by,updated_by,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,1,?6,?6,?7,?7)",
        params![meta.id,meta.crew_id,meta.title,body,body.len() as i64,serde_json::to_string(&meta.created_by)?,meta.created_at])?;
    conn.execute(
        "INSERT INTO document_requests(request_id,document_id,payload_digest)VALUES(?1,?2,?3)",
        params![request, meta.id, digest],
    )?;
    event(conn, meta, Some(body), "created")
}
pub fn update(
    conn: &Connection,
    old: &DocumentSummary,
    title: &str,
    body: &str,
    actor: &ResourceActor,
    at: &str,
) -> Result<bool> {
    legacy_only(conn)?;
    let rev = revision(old)?
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid("document revision exhausted".into()))?;
    let changed=conn.execute("UPDATE documents SET title=?2,body=?3,body_length=?4,revision=?5,updated_by=?6,updated_at=?7 WHERE id=?1 AND revision=?8 AND deleted_at IS NULL",
        params![old.id,title,body,body.len() as i64,rev,serde_json::to_string(actor)?,at,revision(old)?])?;
    if changed == 0 {
        return Ok(false);
    }
    let mut new = old.clone();
    new.title = title.into();
    new.revision = rev.to_string();
    new.updated_by = actor.clone();
    new.updated_at = at.into();
    new.body_length = body.len() as u64;
    event(conn, &new, Some(body), "updated")?;
    Ok(true)
}
pub fn delete(
    conn: &Connection,
    old: &DocumentSummary,
    actor: &ResourceActor,
    at: &str,
) -> Result<bool> {
    legacy_only(conn)?;
    let rev = revision(old)?
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid("document revision exhausted".into()))?;
    let changed=conn.execute("UPDATE documents SET deleted_at=?2,updated_at=?2,updated_by=?3,revision=?4 WHERE id=?1 AND revision=?5 AND deleted_at IS NULL",params![old.id,at,serde_json::to_string(actor)?,rev,revision(old)?])?;
    if changed == 0 {
        return Ok(false);
    }
    let mut new = old.clone();
    new.deleted_at = Some(at.into());
    new.updated_at = at.into();
    new.updated_by = actor.clone();
    new.revision = rev.to_string();
    event(conn, &new, None, "deleted")?;
    Ok(true)
}
fn revision(m: &DocumentSummary) -> Result<i64> {
    m.revision
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == m.revision)
        .ok_or_else(|| StoreError::Invalid("invalid document revision".into()))
}
fn event(conn: &Connection, m: &DocumentSummary, body: Option<&str>, kind: &str) -> Result<()> {
    conn.execute("INSERT INTO document_events(crew_id,document_id,revision,actor,kind,title,body,at)VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![m.crew_id,m.id,revision(m)?,serde_json::to_string(&m.updated_by)?,kind,m.title,body,m.updated_at])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta() -> DocumentSummary {
        let a = ResourceActor {
            kind: "human".into(),
            session_id: None,
        };
        DocumentSummary {
            id: "00000000-0000-0000-0000-000000000010".into(),
            crew_id: "c".into(),
            title: "original".into(),
            revision: "1".into(),
            body_length: 8,
            created_by: a.clone(),
            updated_by: a,
            created_at: "t".into(),
            updated_at: "t".into(),
            deleted_at: None,
        }
    }
    #[test]
    fn staged_v10_document_witness_survives_edits_and_soft_delete_retains_audit_body() {
        let s = crate::testing::open_legacy_store(9).unwrap();
        s.conn()
            .execute(
                "INSERT INTO crews(id,title,created_at,updated_at)VALUES('c','synthetic','t','t')",
                [],
            )
            .unwrap();
        crate::schema::migrate_v10(s.conn()).unwrap();
        let m = meta();
        let tx = s.conn().unchecked_transaction().unwrap();
        insert(&tx, &m, "original", "request", &[7u8; 32]).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            list(s.conn(), "c", None, 51, false).unwrap(),
            vec![m.clone()]
        );
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(update(&tx, &m, "edited", "新正文", &m.updated_by, "t2").unwrap());
        tx.commit().unwrap();
        let edited = get(s.conn(), &m.id).unwrap().unwrap();
        assert_eq!(edited.revision, "2");
        assert_eq!(edited.body_length, 9);
        assert_eq!(
            request(s.conn(), "request").unwrap(),
            Some((m.id.clone(), vec![7; 32]))
        );
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(!update(&tx, &m, "stale", "should not write", &m.updated_by, "bad").unwrap());
        tx.commit().unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(delete(&tx, &edited, &m.updated_by, "t3").unwrap());
        tx.commit().unwrap();
        assert!(list(s.conn(), "c", None, 51, false).unwrap().is_empty());
        assert_eq!(body(s.conn(), &m.id).unwrap().as_deref(), Some("新正文"));
        let originals: Vec<Option<String>> = s
            .conn()
            .prepare("SELECT body FROM document_events ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            originals,
            vec![Some("original".into()), Some("新正文".into()), None]
        );
    }
    #[test]
    fn staged_v10_mid_ddl_failure_keeps_v9_and_rolls_back_new_document_tables() {
        let s = crate::testing::open_legacy_store(9).unwrap();
        s.conn()
            .execute_batch("CREATE TABLE view_folder_scopes(injected INTEGER);")
            .unwrap();
        assert!(crate::schema::migrate_v10(s.conn()).is_err());
        let v: i64 = s
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 9);
        let n:i64=s.conn().query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('documents','document_requests','document_events')",[],|r|r.get(0)).unwrap();
        assert_eq!(n, 0);
        s.conn()
            .execute_batch("DROP TABLE view_folder_scopes;")
            .unwrap();
        crate::schema::migrate_v10(s.conn()).unwrap();
        let v: i64 = s
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 10);
    }
}
