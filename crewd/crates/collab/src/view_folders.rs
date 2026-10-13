//! Independent crew/view classification with live ACL projection, never work graph mutation.
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Sender, Tx, channels, resource_acl as acl};
use crewd_proto::*;
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind},
    view_folders as db,
};
use rusqlite::Connection;
fn invalid(s: &str) -> CollabError {
    acl::invalid(s)
}
fn missing(id: &str) -> CollabError {
    CollabError::NotFound(Refusal::new(Code::ResourceNotFound).arg("id", id))
}
fn canonical(o: &FolderObjectRef) -> Result<FolderObjectRef> {
    let id = match o.kind {
        FolderObjectKind::Channel => {
            if o.id.is_empty() || o.id.len() > 4096 {
                return Err(invalid("object"));
            }
            o.id.clone()
        }
        FolderObjectKind::Document => acl::uuid(&o.id)?,
        k => {
            let prefix = match k {
                FolderObjectKind::Requirement => '#',
                FolderObjectKind::Work => 'W',
                FolderObjectKind::HumanTask => 'H',
                _ => unreachable!(),
            };
            number(&o.id, prefix)?;
            o.id.clone()
        }
    };
    Ok(FolderObjectRef { kind: o.kind, id })
}
fn number(id: &str, prefix: char) -> Result<i64> {
    id.strip_prefix(prefix)
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0 && id == format!("{prefix}{n}"))
        .ok_or_else(|| invalid("object"))
}
fn visible(
    conn: &Connection,
    by: &Sender,
    crew: &str,
    v: FolderView,
    o: &FolderObjectRef,
) -> Result<bool> {
    let allowed = matches!(
        (v, o.kind),
        (FolderView::Messages, FolderObjectKind::Channel)
            | (FolderView::Documents, FolderObjectKind::Document)
            | (FolderView::Progress, FolderObjectKind::Work)
            | (
                FolderView::Todo,
                FolderObjectKind::Work
                    | FolderObjectKind::Requirement
                    | FolderObjectKind::HumanTask
            )
    );
    if !allowed {
        return Ok(false);
    }
    match o.kind {
        FolderObjectKind::Channel => {
            let ch = match channels::get(conn, &o.id) {
                Ok(ch) => ch,
                Err(CollabError::NotFound(_)) => return Ok(false),
                Err(e) => return Err(e),
            };
            if ch.crew_id != crew && ch.kind != ChannelKind::Dm {
                return Ok(false);
            }
            match by {
                Sender::Human => Ok(true),
                Sender::Session(s) => channels::readable_by(conn, &o.id, s),
                Sender::System => Ok(false),
            }
        }
        FolderObjectKind::Document => Ok(crewd_store::documents::get(conn, &o.id)?
            .is_some_and(|m| m.crew_id == crew && m.deleted_at.is_none())),
        k => {
            let (kind, prefix) = match k {
                FolderObjectKind::Requirement => (ObjectKind::Requirement, '#'),
                FolderObjectKind::Work => (ObjectKind::Plan, 'W'),
                FolderObjectKind::HumanTask => (ObjectKind::HumanTask, 'H'),
                _ => unreachable!(),
            };
            Ok(object_refs::get(conn, kind, number(&o.id, prefix)?)?
                .is_some_and(|r| r.crew_id == crew))
        }
    }
}
fn projection(conn: &Connection, by: &Sender, mut s: FolderSnapshot) -> Result<FolderSnapshot> {
    let mut out = Vec::with_capacity(s.assignments.len());
    for a in s.assignments {
        if visible(conn, by, &s.crew_id, s.view, &a.object)? {
            out.push(a)
        }
    }
    s.assignments = out;
    Ok(s)
}
pub fn list(conn: &Connection, by: &Sender, p: FolderListParams) -> Result<FolderSnapshot> {
    acl::actor(conn, by, &p.crew_id, false)?;
    projection(conn, by, db::snapshot(conn, &p.crew_id, p.view)?)
}
fn cas(s: &FolderSnapshot, expected: &str) -> Result<i64> {
    let n = expected
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == expected)
        .ok_or_else(|| invalid("expectedTreeRevision"))?;
    if expected != s.tree_revision {
        let mut r = Refusal::new(Code::FolderRevisionConflict);
        r.current_tree_revision = Some(s.tree_revision.clone());
        return Err(CollabError::Conflict(r));
    }
    Ok(n)
}
fn name(s: &str) -> Result<String> {
    let n = s.trim();
    if n.is_empty() || n.len() > 256 {
        return Err(invalid("name"));
    }
    Ok(n.into())
}
fn parent(raw: Option<&str>, s: &FolderSnapshot) -> Result<Option<String>> {
    let id = raw.map(acl::uuid).transpose()?;
    if let Some(id) = &id
        && !s.folders.iter().any(|f| f.id == *id)
    {
        return Err(missing(id));
    }
    Ok(id)
}
fn validate_tree(folders: &[ViewFolder]) -> Result<()> {
    let map: std::collections::BTreeMap<_, _> = folders.iter().map(|f| (&f.id, f)).collect();
    for f in folders {
        let mut p = f.parent_id.as_ref();
        let mut depth = 1;
        while let Some(id) = p {
            depth += 1;
            if depth > 32 || id == &f.id {
                return Err(CollabError::Conflict(Refusal::new(Code::FolderCycle)));
            }
            p = map
                .get(id)
                .ok_or_else(|| StoreError::Invalid("folder parent not in proposed tree".into()))?
                .parent_id
                .as_ref();
        }
    }
    Ok(())
}
fn commit(
    tx: &mut Tx<'_>,
    by: &Sender,
    s: &FolderSnapshot,
    expected: i64,
    body: serde_json::Value,
) -> Result<FolderSnapshot> {
    let actor = acl::actor(tx.conn(), by, &s.crew_id, true)?;
    let rev = db::commit_change(
        tx.conn(),
        &s.crew_id,
        s.view,
        expected,
        &actor,
        &body,
        &tx.now_ts(),
    )?;
    tx.note(Change::FolderChanged {
        crew_id: s.crew_id.clone(),
        view: s.view,
        tree_revision: rev.to_string(),
    });
    list(
        tx.conn(),
        by,
        FolderListParams {
            crew_id: s.crew_id.clone(),
            view: s.view,
        },
    )
}
pub fn create(tx: &mut Tx<'_>, by: &Sender, p: FolderCreateParams) -> Result<FolderSnapshot> {
    let request = acl::uuid(&p.request_id)?;
    acl::actor(tx.conn(), by, &p.crew_id, false)?;
    let digest = acl::digest(&(
        &p.crew_id,
        p.view,
        &p.name,
        &p.parent_id,
        &p.expected_tree_revision,
    ))?;
    if let Some((crew, _, old)) = db::request(tx.conn(), &request)? {
        if crew != p.crew_id || old != digest {
            return Err(CollabError::Conflict(
                Refusal::new(Code::ResourceRequestConflict).arg("request", &request),
            ));
        }
        return list(
            tx.conn(),
            by,
            FolderListParams {
                crew_id: p.crew_id,
                view: p.view,
            },
        );
    }
    acl::actor(tx.conn(), by, &p.crew_id, true)?;
    let mut s = db::snapshot(tx.conn(), &p.crew_id, p.view)?;
    let expected = cas(&s, &p.expected_tree_revision)?;
    if s.folders.len() >= 256 {
        return Err(CollabError::Conflict(
            Refusal::new(Code::FolderLimit).arg("limit", "256"),
        ));
    }
    let parent_id = parent(p.parent_id.as_deref(), &s)?;
    let f = ViewFolder {
        id: uuid::Uuid::new_v4().to_string(),
        name: name(&p.name)?,
        position: s
            .folders
            .iter()
            .filter(|f| f.parent_id == parent_id)
            .count() as u32,
        parent_id,
    };
    s.folders.push(f.clone());
    validate_tree(&s.folders)?;
    db::create(tx.conn(), &p.crew_id, p.view, &f, &request, &digest)?;
    commit(
        tx,
        by,
        &s,
        expected,
        serde_json::json!({"type":"created","folder":f,"requestId":request}),
    )
}
pub fn update(tx: &mut Tx<'_>, by: &Sender, p: FolderUpdateParams) -> Result<FolderSnapshot> {
    acl::actor(tx.conn(), by, &p.crew_id, true)?;
    let s = db::snapshot(tx.conn(), &p.crew_id, p.view)?;
    let expected = cas(&s, &p.expected_tree_revision)?;
    let id = acl::uuid(&p.id)?;
    let old = s
        .folders
        .iter()
        .find(|f| f.id == id)
        .ok_or_else(|| missing(&id))?;
    let new_parent = parent(p.parent_id.as_deref(), &s)?;
    let mut folders = s.folders.clone();
    folders.retain(|f| f.id != id);
    let mut siblings: Vec<_> = folders
        .iter()
        .filter(|f| f.parent_id == new_parent)
        .cloned()
        .collect();
    siblings.sort_by_key(|f| (f.position, f.id.clone()));
    if p.position as usize > siblings.len() {
        return Err(invalid("position"));
    }
    let f = ViewFolder {
        id: id.clone(),
        name: name(&p.name)?,
        parent_id: new_parent.clone(),
        position: p.position,
    };
    siblings.insert(p.position as usize, f.clone());
    for (i, f) in siblings.iter_mut().enumerate() {
        f.position = i as u32;
    }
    folders.retain(|f| f.parent_id != new_parent);
    folders.extend(siblings);
    if old.parent_id != new_parent {
        let mut indices: Vec<_> = folders
            .iter()
            .enumerate()
            .filter(|(_, f)| f.parent_id == old.parent_id)
            .map(|(i, _)| i)
            .collect();
        indices.sort_by_key(|i| (folders[*i].position, folders[*i].id.clone()));
        for (position, i) in indices.into_iter().enumerate() {
            folders[i].position = position as u32;
        }
    }
    validate_tree(&folders)?;
    if folders.iter().all(|f| s.folders.iter().any(|o| o == f)) {
        return projection(tx.conn(), by, s);
    }
    for f in &folders {
        if !s.folders.iter().any(|o| o == f) {
            db::set(tx.conn(), f)?;
        }
    }
    commit(
        tx,
        by,
        &s,
        expected,
        serde_json::json!({"type":"updated","folder":f}),
    )
}
pub fn delete(tx: &mut Tx<'_>, by: &Sender, p: FolderDeleteParams) -> Result<FolderSnapshot> {
    acl::actor(tx.conn(), by, &p.crew_id, true)?;
    let s = db::snapshot(tx.conn(), &p.crew_id, p.view)?;
    let expected = cas(&s, &p.expected_tree_revision)?;
    let id = acl::uuid(&p.id)?;
    let old = s
        .folders
        .iter()
        .find(|f| f.id == id)
        .ok_or_else(|| missing(&id))?;
    let mut children: Vec<_> = s
        .folders
        .iter()
        .filter(|f| f.parent_id.as_deref() == Some(id.as_str()))
        .cloned()
        .collect();
    children.sort_by_key(|f| (f.position, f.id.clone()));
    let mut siblings: Vec<_> = s
        .folders
        .iter()
        .filter(|f| f.parent_id == old.parent_id && f.id != id)
        .cloned()
        .collect();
    siblings.sort_by_key(|f| (f.position, f.id.clone()));
    let pos = (old.position as usize).min(siblings.len());
    for c in &mut children {
        c.parent_id = old.parent_id.clone();
    }
    siblings.splice(pos..pos, children.clone());
    for (i, f) in siblings.iter_mut().enumerate() {
        f.position = i as u32;
        db::set(tx.conn(), f)?;
    }
    db::delete(tx.conn(), &id)?;
    commit(
        tx,
        by,
        &s,
        expected,
        serde_json::json!({"type":"deleted","folderId":id,"promoted":children.iter().map(|f|&f.id).collect::<Vec<_>>()}),
    )
}
pub fn assign(tx: &mut Tx<'_>, by: &Sender, p: FolderAssignParams) -> Result<FolderSnapshot> {
    acl::actor(tx.conn(), by, &p.crew_id, true)?;
    let s = db::snapshot(tx.conn(), &p.crew_id, p.view)?;
    let expected = cas(&s, &p.expected_tree_revision)?;
    let o = canonical(&p.object)?;
    if !visible(tx.conn(), by, &p.crew_id, p.view, &o)? {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::FolderObjectScope,
        )));
    }
    let folder = parent(p.folder_id.as_deref(), &s)?;
    let current = s.assignments.iter().find(|a| a.object == o);
    if current.map(|a| a.folder_id.as_str()) == folder.as_deref() {
        return projection(tx.conn(), by, s);
    }
    if folder.is_some() && current.is_none() && s.assignments.len() >= 4096 {
        return Err(CollabError::Conflict(
            Refusal::new(Code::FolderLimit).arg("limit", "4096"),
        ));
    }
    db::assign(tx.conn(), &p.crew_id, p.view, &o, folder.as_deref())?;
    commit(
        tx,
        by,
        &s,
        expected,
        serde_json::json!({"type":"assigned","object":o,"folderId":folder}),
    )
}
