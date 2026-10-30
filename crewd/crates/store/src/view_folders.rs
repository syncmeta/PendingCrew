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
        FolderObjectKind::KnowledgeView => "knowledgeView",
        FolderObjectKind::Message => "message",
        FolderObjectKind::Channel => "channel",
        FolderObjectKind::Requirement => "requirement",
        FolderObjectKind::Work => "work",
        FolderObjectKind::HumanTask => "humanTask",
        FolderObjectKind::Document => "document",
    }
}
fn parse_kind(s: String) -> rusqlite::Result<FolderObjectKind> {
    match s.as_str() {
        "knowledgeView" => Ok(FolderObjectKind::KnowledgeView),
        "message" => Ok(FolderObjectKind::Message),
        "channel" => Ok(FolderObjectKind::Channel),
        "requirement" => Ok(FolderObjectKind::Requirement),
        "work" => Ok(FolderObjectKind::Work),
        "humanTask" => Ok(FolderObjectKind::HumanTask),
        "document" => Ok(FolderObjectKind::Document),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
pub fn revision(
    conn: &Connection,
    crew: &str,
    channel: Option<&str>,
    v: FolderView,
) -> Result<i64> {
    let n = conn
        .query_row(
            "SELECT revision FROM view_folder_scopes WHERE crew_id=?1 AND channel_key=?2 AND view=?3",
            params![crew, channel.unwrap_or(""), view(v)],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    if n < 0 {
        return Err(StoreError::Invalid("invalid folder scope revision".into()));
    }
    Ok(n)
}
pub fn snapshot(
    conn: &Connection,
    crew: &str,
    channel: Option<&str>,
    v: FolderView,
) -> Result<FolderSnapshot> {
    let rev = revision(conn, crew, channel, v)?;
    let mut q=conn.prepare("SELECT id,name,parent_id,position FROM view_folders WHERE crew_id=?1 AND channel_key=?2 AND view=?3 ORDER BY COALESCE(parent_id,''),position,id LIMIT 257")?;
    let folders = q
        .query_map(params![crew, channel.unwrap_or(""), view(v)], |r| {
            Ok(ViewFolder {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                position: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut q=conn.prepare("SELECT object_kind,object_id,folder_id,knowledge_source FROM view_folder_assignments WHERE crew_id=?1 AND channel_key=?2 AND view=?3 ORDER BY object_kind,object_id LIMIT 4097")?;
    let assignments = q
        .query_map(params![crew, channel.unwrap_or(""), view(v)], |r| {
            let kind = parse_kind(r.get(0)?)?;
            let raw: String = r.get(1)?;
            let source: Option<String> = r.get(3)?;
            let knowledge = source
                .map(|v| {
                    serde_json::from_str::<crewd_proto::ChannelKnowledgeReference>(&v)
                        .map_err(|_| rusqlite::Error::InvalidQuery)
                })
                .transpose()?;
            let id = if kind == FolderObjectKind::KnowledgeView {
                let s = knowledge.as_ref().ok_or(rusqlite::Error::InvalidQuery)?;
                if crate::knowledge_channels::key(s).map_err(|_| rusqlite::Error::InvalidQuery)?
                    != raw
                {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                s.reference.id.clone()
            } else {
                if knowledge.is_some() {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                raw
            };
            Ok(FolderAssignment {
                object: FolderObjectRef {
                    kind,
                    id,
                    knowledge,
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
        channel_id: channel.map(str::to_owned),
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
    channel: Option<&str>,
    v: FolderView,
    f: &ViewFolder,
    request: &str,
    digest: &[u8],
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO view_folder_scopes(crew_id,channel_key,view,revision)VALUES(?1,?2,?3,0)",
        params![crew, channel.unwrap_or(""), view(v)],
    )?;
    conn.execute("INSERT INTO view_folders(id,crew_id,channel_key,view,name,parent_id,position)VALUES(?1,?2,?3,?4,?5,?6,?7)",params![f.id,crew,channel.unwrap_or(""),view(v),f.name,f.parent_id,f.position])?;
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
    channel: Option<&str>,
    v: FolderView,
    o: &FolderObjectRef,
    folder: Option<&str>,
) -> Result<()> {
    match folder {
        Some(id) => {
            conn.execute("INSERT INTO view_folder_assignments(crew_id,channel_key,view,object_kind,object_id,folder_id,knowledge_source)VALUES(?1,?2,?3,?4,?5,?6,?7)ON CONFLICT(crew_id,channel_key,view,object_kind,object_id)DO UPDATE SET folder_id=excluded.folder_id",params![crew,channel.unwrap_or(""),view(v),kind(o.kind),object_key(o)?,id,o.knowledge.as_ref().map(serde_json::to_string).transpose()?])?;
        }
        None => {
            conn.execute("DELETE FROM view_folder_assignments WHERE crew_id=?1 AND channel_key=?2 AND view=?3 AND object_kind=?4 AND object_id=?5",params![crew,channel.unwrap_or(""),view(v),kind(o.kind),object_key(o)?])?;
        }
    }
    Ok(())
}
#[expect(
    clippy::too_many_arguments,
    reason = "Keep scope, revision and actor audit facts explicit in the caller-owned folder transaction"
)]
pub fn commit_change(
    conn: &Connection,
    crew: &str,
    channel: Option<&str>,
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
        "UPDATE view_folder_scopes SET revision=?4 WHERE crew_id=?1 AND channel_key=?2 AND view=?3 AND revision=?5",
        params![crew,channel.unwrap_or(""),view(v),next,expected],
    )?;
    if changed != 1 {
        return Err(StoreError::Invalid(
            "folder scope changed inside transaction".into(),
        ));
    }
    conn.execute("INSERT INTO view_folder_events(crew_id,channel_key,view,revision,actor,body,at)VALUES(?1,?2,?3,?4,?5,?6,?7)",params![crew,channel.unwrap_or(""),view(v),next,serde_json::to_string(actor)?,body.to_string(),at])?;
    Ok(next)
}

pub fn object_key(o: &FolderObjectRef) -> Result<String> {
    if o.kind == FolderObjectKind::KnowledgeView {
        crate::knowledge_channels::key(
            o.knowledge
                .as_ref()
                .ok_or_else(|| StoreError::Invalid("knowledge reference missing".into()))?,
        )
    } else {
        Ok(o.id.clone())
    }
}
