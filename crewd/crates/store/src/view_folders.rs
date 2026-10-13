//! Folder scope state and history are separate from work graph parentage.
use crate::{Result, StoreError};
use crewd_proto::{
    FolderAssignment, FolderObjectKind, FolderObjectRef, FolderSnapshot, FolderView, ResourceActor,
    ViewFolder,
};
use rusqlite::{Connection, OptionalExtension, params};
pub fn view(v: FolderView) -> &'static str {
    match v {
        FolderView::Messages => "messages",
        FolderView::Todo => "todo",
        FolderView::Documents => "documents",
        FolderView::Progress => "progress",
    }
}
pub fn kind(k: FolderObjectKind) -> &'static str {
    match k {
        FolderObjectKind::Channel => "channel",
        FolderObjectKind::Requirement => "requirement",
        FolderObjectKind::Work => "work",
        FolderObjectKind::HumanTask => "humanTask",
        FolderObjectKind::Document => "document",
    }
}
fn parse_kind(s: String) -> rusqlite::Result<FolderObjectKind> {
    match s.as_str() {
        "channel" => Ok(FolderObjectKind::Channel),
        "requirement" => Ok(FolderObjectKind::Requirement),
        "work" => Ok(FolderObjectKind::Work),
        "humanTask" => Ok(FolderObjectKind::HumanTask),
        "document" => Ok(FolderObjectKind::Document),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
pub fn revision(conn: &Connection, crew: &str, v: FolderView) -> Result<i64> {
    let n = conn
        .query_row(
            "SELECT revision FROM view_folder_scopes WHERE crew_id=?1 AND view=?2",
            params![crew, view(v)],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    if n < 0 {
        return Err(StoreError::Invalid("invalid folder scope revision".into()));
    }
    Ok(n)
}
pub fn snapshot(conn: &Connection, crew: &str, v: FolderView) -> Result<FolderSnapshot> {
    let rev = revision(conn, crew, v)?;
    let mut q=conn.prepare("SELECT id,name,parent_id,position FROM view_folders WHERE crew_id=?1 AND view=?2 ORDER BY COALESCE(parent_id,''),position,id LIMIT 257")?;
    let folders = q
        .query_map(params![crew, view(v)], |r| {
            Ok(ViewFolder {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                position: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut q=conn.prepare("SELECT object_kind,object_id,folder_id FROM view_folder_assignments WHERE crew_id=?1 AND view=?2 ORDER BY object_kind,object_id LIMIT 4097")?;
    let assignments = q
        .query_map(params![crew, view(v)], |r| {
            Ok(FolderAssignment {
                object: FolderObjectRef {
                    kind: parse_kind(r.get(0)?)?,
                    id: r.get(1)?,
                },
                folder_id: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if folders.len() > 256 || assignments.len() > 4096 {
        return Err(StoreError::Invalid(
            "folder scope exceeds bounded state".into(),
        ));
    }
    let ids: std::collections::BTreeMap<_, _> =
        folders.iter().map(|f| (f.id.as_str(), f)).collect();
    for f in &folders {
        let mut p = f.parent_id.as_deref();
        let mut depth = 0;
        while let Some(id) = p {
            depth += 1;
            if depth >= 32 || id == f.id {
                return Err(StoreError::Invalid("folder cycle or depth invalid".into()));
            }
            p = ids
                .get(id)
                .ok_or_else(|| StoreError::Invalid("folder parent outside scope".into()))?
                .parent_id
                .as_deref();
        }
    }
    if assignments
        .iter()
        .any(|a| !ids.contains_key(a.folder_id.as_str()))
    {
        return Err(StoreError::Invalid(
            "folder assignment outside scope".into(),
        ));
    }
    Ok(FolderSnapshot {
        crew_id: crew.into(),
        view: v,
        tree_revision: rev.to_string(),
        folders,
        assignments,
    })
}
pub fn request(conn: &Connection, id: &str) -> Result<Option<(String, String, Vec<u8>)>> {
    let out: Option<(String, String, Vec<u8>)> = conn
        .query_row(
            "SELECT crew_id,folder_id,payload_digest FROM view_folder_requests WHERE request_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if out.as_ref().is_some_and(|(_, _, d)| d.len() != 32) {
        return Err(StoreError::Invalid("invalid folder create witness".into()));
    }
    Ok(out)
}
pub fn create(
    conn: &Connection,
    crew: &str,
    v: FolderView,
    f: &ViewFolder,
    request: &str,
    digest: &[u8],
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO view_folder_scopes(crew_id,view,revision)VALUES(?1,?2,0)",
        params![crew, view(v)],
    )?;
    conn.execute("INSERT INTO view_folders(id,crew_id,view,name,parent_id,position)VALUES(?1,?2,?3,?4,?5,?6)",params![f.id,crew,view(v),f.name,f.parent_id,f.position])?;
    conn.execute("INSERT INTO view_folder_requests(request_id,crew_id,folder_id,payload_digest)VALUES(?1,?2,?3,?4)",params![request,crew,f.id,digest])?;
    Ok(())
}
pub fn set(conn: &Connection, f: &ViewFolder) -> Result<()> {
    let changed = conn.execute(
        "UPDATE view_folders SET name=?2,parent_id=?3,position=?4 WHERE id=?1",
        params![f.id, f.name, f.parent_id, f.position],
    )?;
    if changed != 1 {
        return Err(StoreError::Invalid(
            "folder vanished inside transaction".into(),
        ));
    }
    Ok(())
}
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    if conn.execute("DELETE FROM view_folders WHERE id=?1", [id])? != 1 {
        return Err(StoreError::Invalid(
            "folder vanished inside transaction".into(),
        ));
    }
    Ok(())
}
pub fn assign(
    conn: &Connection,
    crew: &str,
    v: FolderView,
    o: &FolderObjectRef,
    folder: Option<&str>,
) -> Result<()> {
    match folder {
        Some(id) => {
            conn.execute("INSERT INTO view_folder_assignments(crew_id,view,object_kind,object_id,folder_id)VALUES(?1,?2,?3,?4,?5)ON CONFLICT(crew_id,view,object_kind,object_id)DO UPDATE SET folder_id=excluded.folder_id",params![crew,view(v),kind(o.kind),o.id,id])?;
        }
        None => {
            conn.execute("DELETE FROM view_folder_assignments WHERE crew_id=?1 AND view=?2 AND object_kind=?3 AND object_id=?4",params![crew,view(v),kind(o.kind),o.id])?;
        }
    }
    Ok(())
}
pub fn commit_change(
    conn: &Connection,
    crew: &str,
    v: FolderView,
    expected: i64,
    actor: &ResourceActor,
    body: &serde_json::Value,
    at: &str,
) -> Result<i64> {
    let next = expected
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid("folder revision exhausted".into()))?;
    let changed = conn.execute(
        "UPDATE view_folder_scopes SET revision=?3 WHERE crew_id=?1 AND view=?2 AND revision=?4",
        params![crew, view(v), next, expected],
    )?;
    if changed != 1 {
        return Err(StoreError::Invalid(
            "folder scope changed inside transaction".into(),
        ));
    }
    conn.execute("INSERT INTO view_folder_events(crew_id,view,revision,actor,body,at)VALUES(?1,?2,?3,?4,?5,?6)",params![crew,view(v),next,serde_json::to_string(actor)?,body.to_string(),at])?;
    Ok(next)
}
