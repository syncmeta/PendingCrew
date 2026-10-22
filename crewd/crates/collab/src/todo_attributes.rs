//! Human-only source facts and original-request recovery; never wakes an executor.
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Sender, Tx, asks, channel_work, resource_acl as acl};
use crewd_proto::*;
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
    todo_attributes as db,
};
use rusqlite::Connection;
fn human(by: &Sender) -> Result<()> {
    if matches!(by, Sender::Human) {
        Ok(())
    } else {
        Err(CollabError::Forbidden(Refusal::new(Code::TodoHumanOnly)))
    }
}
pub(crate) fn reference(c: &Connection, o: &ChannelWorkObject) -> Result<ObjectReference> {
    let (k, prefix) = match o.kind {
        ChannelWorkKind::Requirement => (ObjectKind::Requirement, '#'),
        ChannelWorkKind::HumanTask => (ObjectKind::HumanTask, 'H'),
        _ => return Err(acl::invalid("object.kind")),
    };
    let n =
        o.id.strip_prefix(prefix)
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|n| *n > 0 && o.id == format!("{prefix}{n}"))
            .ok_or_else(|| acl::invalid("object.id"))?;
    object_refs::get(c, k, n)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::ResourceNotFound).arg("id", &o.id)))
}
pub fn get(c: &Connection, by: &Sender, o: &ChannelWorkObject) -> Result<TodoAttributes> {
    let r = reference(c, o)?;
    acl::actor(c, by, &r.crew_id, false)?;
    if let Some(a) = db::get(c, o)? {
        if a.crew_id != r.crew_id || a.object != *o {
            return Err(StoreError::Invalid("todo source witness mismatch".into()).into());
        }
        return Ok(a);
    }
    Ok(TodoAttributes {
        object: o.clone(),
        crew_id: r.crew_id,
        revision: "0".into(),
        origin: if o.kind == ChannelWorkKind::Requirement {
            TodoOrigin::Requirement
        } else {
            TodoOrigin::AgentAsk
        },
        owner: None,
        manual_state: None,
    })
}
fn authorize(
    c: &Connection,
    by: &Sender,
    ch: &str,
    o: &ChannelWorkObject,
) -> Result<TodoAttributes> {
    human(by)?;
    channel_work::authorize(c, by, ch, true)?;
    let a = get(c, by, o)?;
    acl::actor(c, by, &a.crew_id, true)?;
    if !crewd_store::channel_work::linked(c, ch, o)? {
        return Err(acl::invalid("object.channel"));
    }
    Ok(a)
}
pub fn recovered(
    c: &Connection,
    by: &Sender,
    id: &str,
    operation: &str,
    digest: &[u8],
) -> Result<Option<serde_json::Value>> {
    let Some(m) = db::request(c, id)? else {
        return Ok(None);
    };
    acl::actor(c, by, &m.crew, false)?;
    channel_work::authorize(c, by, &m.channel, false)?;
    if m.operation != operation || m.digest != digest {
        return Err(CollabError::Conflict(
            Refusal::new(Code::ResourceRequestConflict).arg("request", id),
        ));
    }
    Ok(Some(m.result))
}
pub(crate) fn record<T: serde::Serialize>(
    tx: &mut Tx<'_>,
    id: &str,
    operation: &str,
    ch: &str,
    a: &TodoAttributes,
    digest: Vec<u8>,
    result: &T,
) -> Result<()> {
    db::record(
        tx.conn(),
        id,
        &db::Mutation {
            operation: operation.into(),
            crew: a.crew_id.clone(),
            channel: ch.into(),
            object_kind: crewd_store::channel_work::kind(&a.object.kind).into(),
            object_id: a.object.id.clone(),
            digest,
            result: serde_json::to_value(result).map_err(StoreError::from)?,
        },
        &tx.now_ts(),
    )?;
    Ok(())
}
fn cas(a: &TodoAttributes, expected: &str) -> Result<i64> {
    let n = expected
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == expected)
        .ok_or_else(|| acl::invalid("expectedRevision"))?;
    if a.revision != expected {
        let mut refusal = Refusal::new(Code::TodoAttributesRevisionConflict);
        refusal.current_todo_attributes = Some(Box::new(a.clone()));
        return Err(CollabError::Conflict(refusal));
    }
    Ok(n)
}
fn save(tx: &mut Tx<'_>, a: &mut TodoAttributes, expected: i64) -> Result<()> {
    a.revision = expected
        .checked_add(1)
        .ok_or_else(|| acl::invalid("revision"))?
        .to_string();
    db::save(tx.conn(), a, expected, &tx.now_ts())?;
    tx.note(Change::TodoAttributesChanged {
        crew_id: a.crew_id.clone(),
        object: a.object.clone(),
        revision: a.revision.clone(),
    });
    Ok(())
}
pub fn assign(tx: &mut Tx<'_>, by: &Sender, p: TodoAssignParams) -> Result<TodoAttributes> {
    human(by)?;
    let request = acl::uuid(&p.request_id)?;
    let digest = acl::digest(&p)?;
    if let Some(v) = recovered(tx.conn(), by, &request, "todo.assign", &digest)? {
        return Ok(serde_json::from_value(v).map_err(StoreError::from)?);
    }
    let mut a = authorize(tx.conn(), by, &p.channel_id, &p.object)?;
    let rev = cas(&a, &p.expected_revision)?;
    if let Some(MemberRef::Session { id }) = &p.owner {
        if !crate::channels::is_member(
            tx.conn(),
            &p.channel_id,
            &MemberRef::Session { id: id.clone() },
        )? {
            return Err(acl::invalid("owner"));
        }
    }
    a.owner = p.owner;
    save(tx, &mut a, rev)?;
    record(tx, &request, "todo.assign", &p.channel_id, &a, digest, &a)?;
    Ok(a)
}
pub fn set_state(tx: &mut Tx<'_>, by: &Sender, p: TodoSetStateParams) -> Result<TodoAttributes> {
    human(by)?;
    let request = acl::uuid(&p.request_id)?;
    let digest = acl::digest(&p)?;
    if let Some(v) = recovered(tx.conn(), by, &request, "todo.setState", &digest)? {
        return Ok(serde_json::from_value(v).map_err(StoreError::from)?);
    }
    let mut a = authorize(tx.conn(), by, &p.channel_id, &p.object)?;
    let rev = cas(&a, &p.expected_revision)?;
    if a.origin == TodoOrigin::AgentAsk {
        return Err(acl::invalid("origin"));
    }
    if a.origin == TodoOrigin::Requirement {
        let r = reference(tx.conn(), &p.object)?;
        let core = crewd_store::work_organization::core_for_reference(tx.conn(), r.kind, r.number)?
            .ok_or_else(|| acl::invalid("object"))?
            .parse()
            .map_err(|_| acl::invalid("object"))?;
        asks::with_book(tx, &a.crew_id, |b, now| {
            b.set_entry_resolution(
                &crewd_work::Actor::Human,
                now,
                core,
                p.state == ManualTodoState::Done,
            )
        })?;
    }
    a.manual_state = Some(p.state);
    save(tx, &mut a, rev)?;
    record(tx, &request, "todo.setState", &p.channel_id, &a, digest, &a)?;
    Ok(a)
}
pub fn create_human(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: HumanTaskCreateParams,
) -> Result<HumanTaskCreateResult> {
    human(by)?;
    let request = acl::uuid(&p.request_id)?;
    let digest = acl::digest(&p)?;
    if let Some(v) = recovered(tx.conn(), by, &request, "humanTask.create", &digest)? {
        let mut r: HumanTaskCreateResult = serde_json::from_value(v).map_err(StoreError::from)?;
        r.reused = true;
        return Ok(r);
    }
    acl::actor(tx.conn(), by, &p.crew_id, true)?;
    channel_work::authorize(tx.conn(), by, &p.channel_id, true)?;
    if p.text.trim().is_empty() || p.text.len() > 65536 {
        return Err(acl::invalid("text"));
    }
    let id = asks::with_book(tx, &p.crew_id, |b, now| {
        b.create_human_task(
            &crewd_work::Actor::Human,
            now,
            crewd_work::CrewId::new(&p.crew_id),
            &p.text,
        )
    })?
    .value;
    let r = crewd_store::work_organization::reference_for_core(
        tx.conn(),
        ObjectKind::HumanTask,
        &id.to_string(),
    )?
    .ok_or_else(|| acl::invalid("created object"))?;
    let object = ChannelWorkObject {
        kind: ChannelWorkKind::HumanTask,
        id: r.id(),
    };
    let mut a = TodoAttributes {
        object: object.clone(),
        crew_id: p.crew_id.clone(),
        revision: "0".into(),
        origin: TodoOrigin::HumanTask,
        owner: Some(MemberRef::Human),
        manual_state: Some(ManualTodoState::Open),
    };
    save(tx, &mut a, 0)?;
    let association = channel_work::link_created(tx, by, &p.channel_id, object)?;
    let result = HumanTaskCreateResult {
        ask: asks::get(tx.conn(), &p.crew_id, &r.id())?,
        todo: a.clone(),
        channel_association: association,
        request_id: request.clone(),
        reused: false,
    };
    record(
        tx,
        &request,
        "humanTask.create",
        &p.channel_id,
        &a,
        digest,
        &result,
    )?;
    Ok(result)
}
pub fn lookup(
    c: &Connection,
    by: &Sender,
    p: ResourceRequestLookupParams,
) -> Result<ResourceRequestLookupResult> {
    human(by)?;
    let request = acl::uuid(&p.request_id)?;
    let Some(m) = db::request(c, &request)? else {
        return Ok(ResourceRequestLookupResult {
            request_id: request,
            status: "notFound".into(),
            operation: None,
            result: None,
        });
    };
    acl::actor(c, by, &m.crew, false)?;
    channel_work::authorize(c, by, &m.channel, false)?;
    Ok(ResourceRequestLookupResult {
        request_id: request,
        status: "committed".into(),
        operation: Some(m.operation),
        result: Some(m.result),
    })
}
