//! Human UI snapshots are not the agent's default payload. All agent reads are bounded.
use super::{Ctx, parse_args};
use crate::agent::port;
use crewd_proto::{agent::CallResult, *};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
fn result<T: Serialize>(cx: &Ctx<'_>, v: port::PortResult<T>) -> CallResult {
    match v {
        Ok(v) => match serde_json::to_string(&v) {
            Ok(s) => cx.ok("r.resource_result", &[("result", &s)]),
            Err(e) => cx.fail(port::PortError::Internal(e.to_string())),
        },
        Err(e) => cx.fail(e),
    }
}
fn bad(cx: &Ctx<'_>, field: &str) -> CallResult {
    cx.fail(port::PortError::from(crewd_collab::resource_invalid(field)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupList {
    channel_id: String,
    after: Option<String>,
    limit: Option<u16>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupLink {
    channel_id: String,
    expected_link_revision: String,
    object: ChannelWorkObject,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: String,
    crew_id: String,
    title: String,
    body: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    id: String,
    offset: Option<u32>,
    limit: Option<u32>,
    expected_revision: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    crew_id: String,
    after: Option<String>,
    limit: Option<u32>,
    #[serde(default)]
    include_deleted: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    id: String,
    expected_revision: String,
    title: String,
    body: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    id: String,
    expected_revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    id: String,
    expected_revision: String,
    edits: Vec<DocumentEdit>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Part {
    Folders,
    Assignments,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderList {
    crew_id: String,
    view: FolderView,
    part: Option<Part>,
    after: Option<String>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderCreate {
    crew_id: String,
    view: FolderView,
    request_id: String,
    expected_tree_revision: String,
    name: String,
    parent_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderUpdate {
    crew_id: String,
    view: FolderView,
    id: String,
    expected_tree_revision: String,
    name: String,
    parent_id: Option<String>,
    position: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderDelete {
    crew_id: String,
    view: FolderView,
    id: String,
    expected_tree_revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderAssign {
    crew_id: String,
    view: FolderView,
    expected_tree_revision: String,
    object: FolderObjectRef,
    folder_id: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    crew: String,
    view: FolderView,
    part: Part,
    revision: String,
    last: String,
}
fn folder_receipt(s: FolderSnapshot, id: &str) -> Value {
    json!({"crewId":s.crew_id,"view":s.view,"treeRevision":s.tree_revision,"folderId":id})
}
macro_rules! args {
    ($cx:expr,$v:expr,$ty:ty) => {
        match parse_args::<$ty>($cx, $v) {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}
pub async fn call(cx: &Ctx<'_>, tool: &str, v: Value) -> CallResult {
    let session = &cx.me.session_id;
    match tool {
        "channel_work_list" => {
            let a = args!(cx, v, GroupList);
            result(
                cx,
                port::channel_work_list(
                    cx.st,
                    session,
                    ChannelWorkListParams {
                        kinds: None,
                        tag: None,
                        channel_id: a.channel_id,
                        after: a.after,
                        limit: a.limit,
                    },
                )
                .await,
            )
        }
        "channel_work_link" | "channel_work_unlink" => {
            let a = args!(cx, v, GroupLink);
            result(
                cx,
                port::channel_work_link(
                    cx.st,
                    session,
                    ChannelWorkLinkParams {
                        channel_id: a.channel_id,
                        expected_link_revision: a.expected_link_revision,
                        object: a.object,
                    },
                    tool == "channel_work_link",
                )
                .await,
            )
        }
        "document_create" => {
            let a = args!(cx, v, Create);
            result(
                cx,
                port::document_create(
                    cx.st,
                    session,
                    DocumentCreateParams {
                        request_id: a.request_id,
                        crew_id: a.crew_id,
                        title: a.title,
                        body: a.body,
                    },
                )
                .await,
            )
        }
        "document_list" => {
            let a = args!(cx, v, List);
            result(
                cx,
                port::document_list(
                    cx.st,
                    session,
                    DocumentListParams {
                        crew_id: a.crew_id,
                        after: a.after,
                        limit: a.limit,
                        include_deleted: a.include_deleted,
                    },
                )
                .await,
            )
        }
        "document_get" => {
            let a = args!(cx, v, Get);
            let offset = a.offset.unwrap_or(0) as usize;
            let limit = a.limit.unwrap_or(4096) as usize;
            if !(1..=16384).contains(&limit) || offset > 0 && a.expected_revision.is_none() {
                return bad(cx, "offset/limit/expected_revision");
            }
            let d = match port::document_get(cx.st, session, DocumentGetParams { id: a.id }).await {
                Ok(v) => v,
                Err(e) => return cx.fail(e),
            };
            if let Some(rev) = a.expected_revision
                && rev != d.document.revision
            {
                let mut r = crewd_collab::refusal::Refusal::new(
                    crewd_collab::refusal::Code::DocumentRevisionConflict,
                )
                .arg("id", &d.document.id);
                r.current_document = Some(Box::new(d.document));
                return cx.fail(port::PortError::from(crewd_collab::CollabError::Conflict(
                    r,
                )));
            }
            if offset > d.body.len() || !d.body.is_char_boundary(offset) {
                return bad(cx, "offset");
            }
            let mut end = (offset + limit).min(d.body.len());
            while end > offset && !d.body.is_char_boundary(end) {
                end -= 1;
            }
            if end == offset && offset < d.body.len() {
                return bad(cx, "limit");
            }
            let next = if end < d.body.len() { Some(end) } else { None };
            result(
                cx,
                Ok(
                    json!({"document":d.document,"bodyChunk":&d.body[offset..end],"offset":offset,"nextOffset":next}),
                ),
            )
        }
        "document_update" => {
            let a = args!(cx, v, Update);
            result(
                cx,
                port::document_update(
                    cx.st,
                    session,
                    DocumentUpdateParams {
                        id: a.id,
                        expected_revision: a.expected_revision,
                        title: a.title,
                        body: a.body,
                    },
                )
                .await,
            )
        }
        "document_delete" => {
            let a = args!(cx, v, Delete);
            result(
                cx,
                port::document_delete(
                    cx.st,
                    session,
                    DocumentDeleteParams {
                        id: a.id,
                        expected_revision: a.expected_revision,
                    },
                )
                .await,
            )
        }
        "document_patch" => {
            let a = args!(cx, v, Patch);
            result(
                cx,
                port::document_patch(
                    cx.st,
                    session,
                    DocumentPatchParams {
                        id: a.id,
                        expected_revision: a.expected_revision,
                        edits: a.edits,
                    },
                )
                .await,
            )
        }
        "view_folder_list" => {
            let a = args!(cx, v, FolderList);
            let part = a.part.unwrap_or(Part::Folders);
            let limit = a.limit.unwrap_or(50) as usize;
            if !(1..=100).contains(&limit) {
                return bad(cx, "limit");
            }
            let s = match port::folder_list(
                cx.st,
                session,
                FolderListParams {
                    channel_id: None,
                    crew_id: a.crew_id.clone(),
                    view: a.view,
                },
            )
            .await
            {
                Ok(s) => s,
                Err(e) => return cx.fail(e),
            };
            let after = if let Some(raw) = a.after {
                let c: Cursor = match serde_json::from_str(&raw) {
                    Ok(c) => c,
                    Err(_) => return bad(cx, "after"),
                };
                if c.crew != a.crew_id
                    || c.view != a.view
                    || c.part != part
                    || c.revision != s.tree_revision
                {
                    let mut r = crewd_collab::refusal::Refusal::new(
                        crewd_collab::refusal::Code::FolderRevisionConflict,
                    );
                    r.current_tree_revision = Some(s.tree_revision);
                    return cx.fail(port::PortError::from(crewd_collab::CollabError::Conflict(
                        r,
                    )));
                }
                Some(c.last)
            } else {
                None
            };
            let mut rows: Vec<(String, Value)> = match part {
                Part::Folders => s
                    .folders
                    .into_iter()
                    .map(|f| (f.id.clone(), json!(f)))
                    .collect(),
                Part::Assignments => s
                    .assignments
                    .into_iter()
                    .map(|f| {
                        (
                            format!(
                                "{}:{}",
                                crewd_store::view_folders::kind(f.object.kind),
                                f.object.id
                            ),
                            json!(f),
                        )
                    })
                    .collect(),
            };
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            rows.retain(|(id, _)| after.as_ref().is_none_or(|a| id > a));
            let has_next = rows.len() > limit;
            rows.truncate(limit);
            let next = if has_next {
                rows.last()
                    .map(|(last, _)| {
                        serde_json::to_string(&Cursor {
                            crew: s.crew_id.clone(),
                            view: s.view,
                            part,
                            revision: s.tree_revision.clone(),
                            last: last.clone(),
                        })
                    })
                    .transpose()
            } else {
                Ok(None)
            };
            let next = match next {
                Ok(n) => n,
                Err(e) => return cx.fail(port::PortError::Internal(e.to_string())),
            };
            result(
                cx,
                Ok(
                    json!({"crewId":s.crew_id,"view":s.view,"treeRevision":s.tree_revision,"items":rows.into_iter().map(|(_,v)|v).collect::<Vec<_>>(),"next":next}),
                ),
            )
        }
        "view_folder_create" => {
            let a = args!(cx, v, FolderCreate);
            result(
                cx,
                port::folder_create_receipt(
                    cx.st,
                    session,
                    FolderCreateParams {
                        channel_id: None,
                        crew_id: a.crew_id,
                        view: a.view,
                        request_id: a.request_id,
                        expected_tree_revision: a.expected_tree_revision,
                        name: a.name,
                        parent_id: a.parent_id,
                    },
                )
                .await,
            )
        }
        "view_folder_update" => {
            let a = args!(cx, v, FolderUpdate);
            let id = a.id.clone();
            result(
                cx,
                port::folder_update(
                    cx.st,
                    session,
                    FolderUpdateParams {
                        channel_id: None,
                        crew_id: a.crew_id,
                        view: a.view,
                        id: a.id,
                        expected_tree_revision: a.expected_tree_revision,
                        name: a.name,
                        parent_id: a.parent_id,
                        position: a.position,
                    },
                )
                .await
                .map(|s| folder_receipt(s, &id)),
            )
        }
        "view_folder_delete" => {
            let a = args!(cx, v, FolderDelete);
            let id = a.id.clone();
            result(
                cx,
                port::folder_delete(
                    cx.st,
                    session,
                    FolderDeleteParams {
                        channel_id: None,
                        crew_id: a.crew_id,
                        view: a.view,
                        id: a.id,
                        expected_tree_revision: a.expected_tree_revision,
                    },
                )
                .await
                .map(|s| {
                    let mut v = folder_receipt(s, &id);
                    v["deleted"] = json!(true);
                    v
                }),
            )
        }
        "view_folder_assign" => {
            let a = args!(cx, v, FolderAssign);
            let o = a.object.clone();
            let folder = a.folder_id.clone();
            result(cx,port::folder_assign(cx.st,session,FolderAssignParams{
channel_id: None,crew_id:a.crew_id,view:a.view,expected_tree_revision:a.expected_tree_revision,object:a.object,folder_id:a.folder_id}).await.map(|s|json!({"crewId":s.crew_id,"view":s.view,"treeRevision":s.tree_revision,"object":o,"folderId":folder})))
        }
        _ => bad(cx, "tool"),
    }
}
