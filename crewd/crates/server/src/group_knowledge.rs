//! Explicit saved-knowledge association. Core owns references, not copied content.
use crate::{AppState, public_commands::Actor};
use crewd_collab::{self as collab, Sender, Tx};
use crewd_proto::{team_data::*, *};
use crewd_store::{
    channel_work as scopes, knowledge_channels as links, team_data::Knowledge,
    todo_attributes as receipts,
};
use std::{path::Path, sync::Arc};
pub(crate) const METHODS: &[&str] = &[
    "channel.knowledgeList",
    "channel.knowledgeLink",
    "channel.knowledgeUnlink",
];
fn invalid(s: &str) -> collab::CollabError {
    collab::resource_invalid(s)
}
fn stable(a: &DataContext, b: &DataContext) -> bool {
    a.db_id == b.db_id
        && a.authority_id == b.authority_id
        && a.restore_epoch == b.restore_epoch
        && a.crew_id == b.crew_id
        && a.namespace == b.namespace
}
fn knowledge(
    c: &rusqlite::Connection,
    root: &Path,
    s: &ChannelKnowledgeReference,
) -> collab::Result<Knowledge> {
    let db = crewd_store::meta_get(c, "dbId")?.ok_or_else(|| invalid("database"))?;
    let namespace = crewd_store::team_data::namespace(&db, &s.context.crew_id);
    if s.context.db_id != db
        || s.context.authority_id != db
        || s.context.namespace != namespace
        || s.reference.namespace != namespace
        || s.reference.kind != "view"
    {
        return Err(invalid("knowledge source"));
    }
    if uuid::Uuid::parse_str(&s.reference.id)
        .ok()
        .is_none_or(|id| id.to_string() != s.reference.id)
    {
        return Err(invalid("reference.id"));
    }
    let base = DataContext {
        db_id: db.clone(),
        authority_id: db,
        restore_epoch: "initial".into(),
        crew_id: s.context.crew_id.clone(),
        namespace: namespace.clone(),
        generation: "0".into(),
    };
    Ok(Knowledge::open(
        &root.join(format!("{namespace}.sqlite")),
        base,
        false,
    )?)
}
pub(crate) async fn handle(state: Arc<AppState>, actor: Actor, request: Request) -> Response {
    let id = request.id;
    let Some(runtime) = state.data_runtime.get() else {
        return Response::err(
            id,
            RpcError::new(RpcError::FORBIDDEN, "SQLite knowledge runtime unavailable"),
        );
    };
    let root = runtime.knowledge_root();
    let owned = state.clone();
    let result = if request.method == "channel.knowledgeList" {
        match parse_params::<ChannelKnowledgeListParams>(request.params) {
            Err(e) => Err(e),
            Ok(p) => state
                .db
                .call_result(move |store| {
                    actor.authorize_tx(&owned, store.conn(), "data.workspace.get")?;
                    list(store.conn(), &root, &p)
                })
                .await
                .map_err(crate::dispatch::internal)
                .and_then(|v| v.map_err(crate::dispatch::collab_err))
                .and_then(|v| serde_json::to_value(v).map_err(crate::dispatch::internal)),
        }
    } else {
        let op = request.method;
        match parse_params::<ChannelKnowledgeLinkParams>(request.params) {
            Err(e) => Err(e),
            Ok(p) => state
                .collab(move |tx| {
                    actor.authorize_tx(&owned, tx.conn(), "data.submit")?;
                    let result = change(tx, &root, &p, &op)?;
                    actor.authorize_tx(&owned, tx.conn(), "data.submit")?;
                    Ok(result)
                })
                .await
                .map_err(crate::dispatch::collab_err)
                .and_then(|v| serde_json::to_value(v).map_err(crate::dispatch::internal)),
        }
    };
    match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}
fn change(
    tx: &mut Tx<'_>,
    root: &Path,
    p: &ChannelKnowledgeLinkParams,
    operation: &str,
) -> collab::Result<ChannelKnowledgeReceipt> {
    let request = uuid::Uuid::parse_str(&p.request_id)
        .map_err(|_| invalid("requestId"))?
        .to_string();
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        &serde_json::to_vec(p).map_err(crewd_store::StoreError::from)?,
    )
    .as_ref()
    .to_vec();
    if let Some(v) =
        collab::todo_attributes::recovered(tx.conn(), &Sender::Human, &request, operation, &digest)?
    {
        return Ok(serde_json::from_value(v).map_err(crewd_store::StoreError::from)?);
    }
    let actor = collab::channel_work::authorize(tx.conn(), &Sender::Human, &p.channel_id, true)?;
    collab::team_data::authorize(tx.conn(), &Sender::Human, &p.source.context.crew_id, false)?;
    let linked = operation == "channel.knowledgeLink";
    let db = crewd_store::meta_get(tx.conn(), "dbId")?.ok_or_else(|| invalid("database"))?;
    if p.source.context.db_id != db
        || p.source.context.authority_id != db
        || p.source.context.namespace
            != crewd_store::team_data::namespace(&db, &p.source.context.crew_id)
        || p.source.reference.namespace != p.source.context.namespace
        || p.source.reference.kind != "view"
        || uuid::Uuid::parse_str(&p.source.reference.id)
            .ok()
            .is_none_or(|id| id.to_string() != p.source.reference.id)
    {
        return Err(invalid("knowledge source identity"));
    }

    if linked {
        let k = knowledge(tx.conn(), root, &p.source)?;
        k.check(&p.source.context)?;
        match k.get(&p.source.reference, None)?.object {
            DataObject::View {
                archived: false, ..
            } => {}
            _ => return Err(invalid("view")),
        };
    }
    let rev = scopes::revision(tx.conn(), &p.channel_id)?;
    if p.expected_link_revision != rev.to_string() {
        let mut r =
            collab::refusal::Refusal::new(collab::refusal::Code::ChannelWorkRevisionConflict);
        r.current_link_revision = Some(Box::new(rev.to_string()));
        return Err(collab::CollabError::Conflict(r));
    }
    let exists = links::linked(tx.conn(), &p.channel_id, &p.source)?;
    let mut next = rev;
    if exists != linked {
        if linked
            && links::rows(tx.conn(), &p.channel_id)?.len()
                + scopes::rows(tx.conn(), &p.channel_id)?.len()
                >= 4096
        {
            return Err(invalid("group references"));
        }
        next = links::change(
            tx.conn(),
            &p.channel_id,
            &p.source,
            rev,
            linked,
            &actor,
            &tx.now_ts(),
        )?;
        tx.note(collab::Change::ChannelWorkChanged {
            channel_id: p.channel_id.clone(),
            link_revision: next.to_string(),
        });
    }
    let result = ChannelKnowledgeReceipt {
        channel_id: p.channel_id.clone(),
        link_revision: next.to_string(),
        source: p.source.clone(),
        linked,
    };
    receipts::record(
        tx.conn(),
        &request,
        &receipts::Mutation {
            operation: operation.into(),
            crew: p.source.context.crew_id.clone(),
            channel: p.channel_id.clone(),
            object_kind: "knowledgeView".into(),
            object_id: links::key(&p.source)?,
            digest,
            result: serde_json::to_value(&result).map_err(crewd_store::StoreError::from)?,
        },
        &tx.now_ts(),
    )?;
    Ok(result)
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    channel: String,
    link: i64,
    source: String,
    last: String,
}
fn list(
    c: &rusqlite::Connection,
    root: &Path,
    p: &ChannelKnowledgeListParams,
) -> collab::Result<ChannelKnowledgePage> {
    collab::channel_work::authorize(c, &Sender::Human, &p.channel_id, false)?;
    let limit = p.limit.unwrap_or(50);
    if limit == 0 || limit > 100 {
        return Err(invalid("limit"));
    }
    let rev = scopes::revision(c, &p.channel_id)?;
    let mut items = Vec::new();
    let mut opened = std::collections::BTreeMap::<String, Option<Knowledge>>::new();
    for source in links::rows(c, &p.channel_id)? {
        collab::team_data::authorize(c, &Sender::Human, &source.context.crew_id, false)?;
        let mut item = ChannelKnowledgeSummary {
            source: source.clone(),
            state: "unavailable".into(),
            view_kind: None,
            title: None,
            revision: None,
            archived: None,
        };
        let namespace = source.context.namespace.clone();
        if !opened.contains_key(&namespace) {
            opened.insert(namespace.clone(), knowledge(c, root, &source).ok());
        }
        if let Some(Some(k)) = opened.get(&namespace) {
            let current = k.context()?;
            if !stable(&source.context, &current) {
                item.state = "contextChanged".into();
            } else if let Ok(stored) = k.get(&source.reference, None) {
                if let DataObject::View {
                    title,
                    revision,
                    archived,
                    definition,
                    ..
                } = stored.object
                {
                    item.state = "ready".into();
                    item.view_kind = Some(
                        match definition {
                            DataViewDefinition::Document { .. } => "document",
                            DataViewDefinition::Web { .. } => "web",
                        }
                        .into(),
                    );
                    item.source.context = current;
                    item.title = Some(title);
                    item.revision = Some(revision);
                    item.archived = Some(archived);
                }
            }
        }
        items.push(item);
    }
    let source = crewd_store::team_data::digest(&items)?;
    let cursor = p
        .after
        .as_deref()
        .map(|s| serde_json::from_str::<Cursor>(s).map_err(|_| invalid("after")))
        .transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|a| a.channel != p.channel_id || a.link != rev || a.source != source)
    {
        return Err(collab::CollabError::Conflict(
            collab::refusal::Refusal::new(collab::refusal::Code::ChannelWorkCursorExpired),
        ));
    }
    items.retain(|s| {
        cursor
            .as_ref()
            .is_none_or(|a| links::key(&s.source).is_ok_and(|k| k > a.last))
    });
    let more = items.len() > usize::from(limit);
    items.truncate(usize::from(limit));
    let next = if more {
        items
            .last()
            .map(|s| {
                Ok::<_, crewd_store::StoreError>(serde_json::to_string(&Cursor {
                    channel: p.channel_id.clone(),
                    link: rev,
                    source: source.clone(),
                    last: links::key(&s.source)?,
                })?)
            })
            .transpose()?
    } else {
        None
    };
    Ok(ChannelKnowledgePage {
        channel_id: p.channel_id.clone(),
        link_revision: rev.to_string(),
        source_revision: source,
        items,
        next,
    })
}
