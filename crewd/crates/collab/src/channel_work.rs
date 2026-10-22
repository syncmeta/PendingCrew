//! Channel references expose current organization facts; linking never changes a work lifecycle.
use crate::refusal::{Code, Refusal};
use crate::{
    Change, CollabError, Result, Sender, Tx, asks, channels, crews, resource_acl as acl, sessions,
};
use crewd_proto::*;
use crewd_store::{
    StoreError, channel_work as db,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_work::{AskId, AskState, EntryNo, EntryStatus, Status, WorkBook};
use rusqlite::Connection;
fn denied() -> CollabError {
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}
pub fn authorize(c: &Connection, by: &Sender, ch: &str, write: bool) -> Result<ResourceActor> {
    let channel = channels::get(c, ch)?;
    if write {
        crews::refuse_archived(c, &channel.crew_id)?;
        if channel.archived_at.is_some() {
            return Err(CollabError::Conflict(
                Refusal::new(Code::ChannelArchived).arg("channel", ch),
            ));
        }
    }
    match by {
        Sender::Human => Ok(ResourceActor {
            kind: "human".into(),
            session_id: None,
        }),
        Sender::Session(id) => {
            let s = sessions::get(c, id)?;
            if write {
                crews::refuse_archived(c, &s.crew_id)?;
            }
            if !channels::readable_by(c, ch, id)? {
                return Err(denied());
            }
            Ok(ResourceActor {
                kind: "session".into(),
                session_id: Some(id.clone()),
            })
        }
        Sender::System => Err(denied()),
    }
}
fn identity(c: &Connection, o: &ChannelWorkObject) -> Result<ObjectReference> {
    let (k, prefix) = match o.kind {
        ChannelWorkKind::Requirement => (ObjectKind::Requirement, '#'),
        ChannelWorkKind::Work => (ObjectKind::Plan, 'W'),
        ChannelWorkKind::HumanTask => (ObjectKind::HumanTask, 'H'),
    };
    let n =
        o.id.strip_prefix(prefix)
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|n| *n > 0 && o.id == format!("{prefix}{n}"))
            .ok_or_else(|| acl::invalid("object.id"))?;
    let reference = object_refs::get(c, k, n)?.ok_or_else(|| {
        CollabError::NotFound(Refusal::new(Code::ResourceNotFound).arg("id", &o.id))
    })?;
    Ok(reference)
}
fn conflict(code: Code, link: i64, source: Option<u64>) -> CollabError {
    let mut r = Refusal::new(code);
    r.current_link_revision = Some(Box::new(link.to_string()));
    r.current_source_revision = source.map(|n| Box::new(n.to_string()));
    CollabError::Conflict(r)
}
fn expected(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == raw)
        .ok_or_else(|| acl::invalid("expectedLinkRevision"))
}
pub fn association(
    c: &Connection,
    by: &Sender,
    ch: &str,
    o: &ChannelWorkObject,
) -> Result<ChannelWorkAssociation> {
    authorize(c, by, ch, false)?;
    let source = identity(c, o)?;
    acl::actor(c, by, &source.crew_id, false)?;
    Ok(ChannelWorkAssociation {
        channel_id: ch.into(),
        link_revision: db::revision(c, ch)?.to_string(),
        linked: db::linked(c, ch, o)?,
    })
}
pub fn link(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: ChannelWorkLinkParams,
    linked: bool,
) -> Result<ChannelWorkLinkReceipt> {
    let actor = authorize(tx.conn(), by, &p.channel_id, true)?;
    let r = identity(tx.conn(), &p.object)?;
    acl::actor(tx.conn(), by, &r.crew_id, false)?;
    let rev = db::revision(tx.conn(), &p.channel_id)?;
    if expected(&p.expected_link_revision)? != rev {
        return Err(conflict(Code::ChannelWorkRevisionConflict, rev, None));
    }
    let mut next = rev;
    if db::linked(tx.conn(), &p.channel_id, &p.object)? != linked {
        if linked
            && db::rows(tx.conn(), &p.channel_id)?.len()
                + crewd_store::knowledge_channels::rows(tx.conn(), &p.channel_id)?.len()
                >= 4096
        {
            return Err(CollabError::Invalid(
                Refusal::new(Code::FolderLimit).arg("limit", "4096"),
            ));
        }
        next = db::change(
            tx.conn(),
            &p.channel_id,
            rev,
            db::LinkChange {
                object: &p.object,
                source_crew: &r.crew_id,
                linked,
                actor: &actor,
                at: &tx.now_ts(),
            },
        )?;
        tx.note(Change::ChannelWorkChanged {
            channel_id: p.channel_id.clone(),
            link_revision: next.to_string(),
        });
    }
    Ok(ChannelWorkLinkReceipt {
        channel_id: p.channel_id,
        link_revision: next.to_string(),
        object: p.object,
        linked,
    })
}
pub fn link_created(
    tx: &mut Tx<'_>,
    by: &Sender,
    ch: &str,
    o: ChannelWorkObject,
) -> Result<ChannelWorkAssociation> {
    let revision = db::revision(tx.conn(), ch)?.to_string();
    let r = link(
        tx,
        by,
        ChannelWorkLinkParams {
            channel_id: ch.into(),
            expected_link_revision: revision,
            object: o,
        },
        true,
    )?;
    Ok(ChannelWorkAssociation {
        channel_id: r.channel_id,
        link_revision: r.link_revision,
        linked: r.linked,
    })
}
fn summary(c: &Connection, b: &WorkBook, o: ChannelWorkObject) -> Result<ChannelWorkSummary> {
    let r = identity(c, &o)?;
    let core = crewd_store::work_organization::core_for_reference(c, r.kind, r.number)?
        .ok_or_else(|| StoreError::Invalid("channel reference lacks core identity".into()))?;
    let todo = if o.kind == ChannelWorkKind::Work {
        None
    } else {
        Some(crate::todo_attributes::get(c, &Sender::Human, &o)?)
    };
    let (label, status, revision) = match o.kind {
        ChannelWorkKind::Work => {
            let id = core.parse().map_err(|_| acl::invalid("work"))?;
            let w = b
                .state()
                .items
                .get(&id)
                .ok_or_else(|| acl::invalid("work"))?;
            (
                w.title.clone(),
                match w.status {
                    Status::Open => "open",
                    Status::Active => "active",
                    Status::Blocked => "blocked",
                    Status::Done => "done",
                    Status::Dropped => "dropped",
                },
                Some(w.revision.to_string()),
            )
        }
        ChannelWorkKind::Requirement => {
            let n: EntryNo = core.parse().map_err(|_| acl::invalid("requirement"))?;
            let v = b.entry_view(n).ok_or_else(|| acl::invalid("requirement"))?;
            let text:String=c.query_row("SELECT substr(t.text,1,121) FROM todos t JOIN object_refs r ON r.raw_todo_id=t.id WHERE r.number=?1 AND r.kind='requirement'",[r.number],|row|row.get(0))?;
            let status = if v.status == EntryStatus::Closed {
                "closed"
            } else {
                match todo.as_ref().and_then(|t| t.manual_state) {
                    Some(ManualTodoState::Open) => "open",
                    Some(ManualTodoState::Done) => "done",
                    None => match v.status {
                        EntryStatus::Unclaimed => "unclaimed",
                        EntryStatus::InProgress => "in_progress",
                        EntryStatus::Done => "done",
                        EntryStatus::NeedsRuling => "needs_ruling",
                        EntryStatus::Closed => "closed",
                    },
                }
            };
            (text, status, todo.as_ref().map(|t| t.revision.clone()))
        }
        ChannelWorkKind::HumanTask => {
            let id: AskId = core.parse().map_err(|_| acl::invalid("humanTask"))?;
            let h = b.ask(id).ok_or_else(|| acl::invalid("humanTask"))?;
            let status = match todo.as_ref().and_then(|t| t.manual_state) {
                Some(ManualTodoState::Open) => "open",
                Some(ManualTodoState::Done) => "done",
                None => match h.state {
                    AskState::Open => "open",
                    AskState::Answered { .. } => "answered",
                    AskState::Defaulted { .. } => "defaulted",
                    AskState::Withdrawn { .. } => "withdrawn",
                },
            };
            (
                h.question.clone(),
                status,
                todo.as_ref().map(|t| t.revision.clone()),
            )
        }
    };
    Ok(ChannelWorkSummary {
        object: o,
        crew_id: r.crew_id,
        label: label.chars().take(120).collect(),
        label_truncated: label.chars().nth(120).is_some(),
        status: status.into(),
        revision,
        tags: vec![],
        todo,
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    channel_id: String,
    link_revision: i64,
    source_revision: String,
    last: String,
    kinds: Option<Vec<ChannelWorkKind>>,
    tag: Option<String>,
}
fn key(o: &ChannelWorkObject) -> String {
    format!("{}:{}", db::kind(&o.kind), o.id)
}
pub fn list(c: &Connection, by: &Sender, p: ChannelWorkListParams) -> Result<ChannelWorkPage> {
    authorize(c, by, &p.channel_id, false)?;
    let limit = p.limit.unwrap_or(50);
    if limit == 0 || limit > 100 {
        return Err(acl::invalid("limit"));
    }
    if p.kinds
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 3)
    {
        return Err(acl::invalid("kinds"));
    }
    if p.tag
        .as_ref()
        .is_some_and(|s| s.trim() != s || s.is_empty() || s.len() > 64)
    {
        return Err(acl::invalid("tag"));
    }
    let rev = db::revision(c, &p.channel_id)?;
    let b = asks::load_org(c)?;
    let attrs: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM todo_attribute_events",
        [],
        |r| r.get(0),
    )?;
    let source = acl::digest(&(b.state().last_seq, attrs))?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let cursor = p
        .after
        .as_deref()
        .map(|s| serde_json::from_str::<Cursor>(s).map_err(|_| acl::invalid("after")))
        .transpose()?;
    if cursor.as_ref().is_some_and(|a| {
        a.channel_id != p.channel_id
            || a.link_revision != rev
            || a.source_revision != source
            || a.kinds != p.kinds
            || a.tag != p.tag
    }) {
        return Err(conflict(Code::ChannelWorkCursorExpired, rev, None));
    }
    let mut items = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    for o in db::rows(c, &p.channel_id)? {
        if p.kinds.as_ref().is_some_and(|v| !v.contains(&o.kind)) {
            continue;
        }
        let source_identity = identity(c, &o)?;
        match acl::actor(c, by, &source_identity.crew_id, false) {
            Ok(_) => {}
            Err(CollabError::Forbidden(_)) => continue,
            Err(e) => return Err(e),
        }
        let mut s = summary(c, &b, o)?;
        s.tags = db::tags(c, &p.channel_id, &s.object)?;
        for tag in &s.tags {
            *counts.entry(tag.clone()).or_default() += 1;
        }
        items.push(s);
    }
    let total = items.len() as u64;
    items.retain(|s| {
        p.tag.as_ref().is_none_or(|t| s.tags.contains(t))
            && cursor.as_ref().is_none_or(|a| key(&s.object) > a.last)
    });
    let more = items.len() > usize::from(limit);
    items.truncate(usize::from(limit));
    let next = if more {
        items
            .last()
            .map(|s| {
                serde_json::to_string(&Cursor {
                    channel_id: p.channel_id.clone(),
                    link_revision: rev,
                    source_revision: source.clone(),
                    last: key(&s.object),
                    kinds: p.kinds,
                    tag: p.tag,
                })
            })
            .transpose()
            .map_err(StoreError::from)?
    } else {
        None
    };
    Ok(ChannelWorkPage {
        channel_id: p.channel_id,
        link_revision: rev.to_string(),
        source_revision: source,
        items,
        next,
        tag_counts: counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect(),
        total,
    })
}
pub fn set_tags(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: ChannelWorkTagsSetParams,
) -> Result<ChannelWorkTagsReceipt> {
    if !matches!(by, Sender::Human) {
        return Err(CollabError::Forbidden(Refusal::new(Code::TodoHumanOnly)));
    }
    let request = acl::uuid(&p.request_id)?;
    let digest = acl::digest(&p)?;
    if let Some(v) =
        crate::todo_attributes::recovered(tx.conn(), by, &request, "channel.workTagsSet", &digest)?
    {
        return Ok(serde_json::from_value(v).map_err(StoreError::from)?);
    }
    let actor = authorize(tx.conn(), by, &p.channel_id, true)?;
    let a = crate::todo_attributes::get(tx.conn(), by, &p.object)?;
    acl::actor(tx.conn(), by, &a.crew_id, true)?;
    if !db::linked(tx.conn(), &p.channel_id, &p.object)? {
        return Err(acl::invalid("object.channel"));
    }
    let rev = db::revision(tx.conn(), &p.channel_id)?;
    if expected(&p.expected_link_revision)? != rev {
        return Err(conflict(Code::ChannelWorkRevisionConflict, rev, None));
    }
    let mut tags = Vec::<String>::new();
    if p.tags.len() > 16 {
        return Err(acl::invalid("tags"));
    }
    for tag in p.tags {
        let tag = tag.trim();
        if tag.is_empty() || tag.len() > 64 {
            return Err(acl::invalid("tags"));
        }
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.into());
        }
    }
    let next = db::set_tags(
        tx.conn(),
        &p.channel_id,
        rev,
        &p.object,
        &tags,
        &actor,
        &tx.now_ts(),
    )?;
    tx.note(Change::ChannelWorkChanged {
        channel_id: p.channel_id.clone(),
        link_revision: next.to_string(),
    });
    let result = ChannelWorkTagsReceipt {
        channel_id: p.channel_id.clone(),
        link_revision: next.to_string(),
        object: p.object,
        tags,
    };
    crate::todo_attributes::record(
        tx,
        &request,
        "channel.workTagsSet",
        &p.channel_id,
        &a,
        digest,
        &result,
    )?;
    Ok(result)
}
