//! Explicit captures share normal object creation, authorization and events.
#[cfg(test)]
#[path = "managed_message_sources_tests.rs"]
mod tests;

use crate::{
    CollabError, Result, Tx, channels,
    managed_objects::{self as objects, Actor},
};
use crewd_proto::{
    Message,
    managed_message_sources::*,
    managed_objects::{Change, Object, Scope, Submit},
};
use crewd_store::managed_message_sources as db;
use rusqlite::Connection;

fn invalid(field: &str) -> CollabError {
    crate::resource_acl::invalid(field)
}
fn forbidden() -> CollabError {
    CollabError::Forbidden(crate::refusal::Refusal::new(
        crate::refusal::Code::ResourceForbidden,
    ))
}
fn conflict(request: &str) -> CollabError {
    CollabError::Conflict(
        crate::refusal::Refusal::new(crate::refusal::Code::ResourceRequestConflict)
            .arg("request", request),
    )
}

fn message(c: &Connection, actor: Actor<'_>, id: &str) -> Result<Message> {
    actor.principal(c)?;
    let message = crewd_store::messages::get(c, id)?.ok_or_else(forbidden)?;
    if let Actor::Session(session) = actor {
        if !channels::readable_by(c, &message.channel_id, session)? {
            return Err(forbidden());
        }
    }
    Ok(message)
}

fn object_id(object: &Object) -> &str {
    match object {
        Object::Table { table } => &table.id,
        Object::Row { row } => &row.id,
        Object::Decision { decision } => &decision.id,
        Object::Risk { risk } => &risk.id,
        Object::Lesson { lesson } => &lesson.id,
    }
}

fn number(id: &str) -> Result<i64> {
    let value = id.get(1..).ok_or_else(|| invalid("id"))?;
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or_else(|| invalid("id"))
}

pub fn source(c: &Connection, actor: Actor<'_>, id: &str) -> Result<Source> {
    objects::get_with_defaults(c, actor, id, true)?;
    let Some(saved) = db::get(c, number(id)?)? else {
        return Ok(Source::None);
    };
    if let Actor::Session(session) = actor {
        let allowed = match saved.channel_id.as_deref() {
            Some(channel) => channels::readable_by(c, channel, session)?,
            None => false,
        };
        if !allowed {
            return Ok(Source::Restricted);
        }
    }
    Ok(match (saved.message_id, saved.channel_id) {
        (Some(message_id), Some(channel_id)) => Source::Available {
            crew_id: channels::get(c, &channel_id)?.crew_id,
            message_id,
            channel_id,
            excerpt: saved.excerpt,
        },
        _ => Source::Deleted {
            excerpt: saved.excerpt,
        },
    })
}

pub fn objects_from_message(
    c: &Connection,
    actor: Actor<'_>,
    id: &str,
    after: Option<&str>,
) -> Result<MessageObjectsPage> {
    message(c, actor, id)?;
    let cursor = match after {
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0 && n.to_string() == value)
            .ok_or_else(|| invalid("after"))?,
        None => 0,
    };
    let mut numbers = db::numbers(c, id, cursor, 17)?;
    let next_after = (numbers.len() > 16).then(|| numbers[15].to_string());
    numbers.truncate(16);
    let mut items = Vec::with_capacity(numbers.len());
    for number in numbers {
        // Each persisted capture has exactly one registered typed identity.
        let object: String = c.query_row(
            "SELECT CASE kind WHEN 'decision' THEN 'D' WHEN 'risk' THEN 'R' WHEN 'lesson' THEN 'L' WHEN 'custom_row' THEN 'O' END || number FROM object_refs WHERE number=?1",
            [number],|row| row.get(0),
        )?;
        items.push(objects::get_with_defaults(c, actor, &object, true)?);
    }
    Ok(MessageObjectsPage { items, next_after })
}

pub fn record(tx: &mut Tx<'_>, actor: Actor<'_>, request: RecordParams) -> Result<RecordReceipt> {
    if !crewd_proto::valid_remote_uuid(&request.request_id) {
        return Err(invalid("requestId"));
    }
    let c = tx.conn();
    let principal = actor.owner(c, &request.crew_id, true)?;
    if objects::scope(c)? != request.scope {
        return Err(conflict(&request.request_id));
    }
    let original = message(c, actor, &request.source_message_id)?;
    let source_type = match &request.change {
        Change::RecordDecision { .. } => "decision".into(),
        Change::RecordRisk { .. } => "risk".into(),
        Change::RecordLesson { .. } => "lesson".into(),
        Change::AddRow { table_id, .. } => {
            let target = objects::get_with_defaults(c, actor, table_id, true)?;
            if target.crew_id() != request.crew_id {
                return Err(forbidden());
            }
            format!("row:{table_id}")
        }
        _ => return Err(invalid("change")),
    };
    let digest: [u8; 32] = crate::resource_acl::digest(&request)?
        .try_into()
        .map_err(|_| invalid("request"))?;
    if let Some((saved, receipt)) =
        db::receipt(c, &request.crew_id, &principal, &request.request_id)?
    {
        return if saved == digest && receipt.scope == request.scope {
            Ok(receipt)
        } else {
            Err(conflict(&request.request_id))
        };
    }
    // A direct object request is a distinct operation and cannot acquire a
    // message source retroactively by reusing its identifier.
    if crewd_store::managed_objects::receipt(c, &request.crew_id, &principal, &request.request_id)?
        .is_some()
    {
        return Err(conflict(&request.request_id));
    }
    let existing = db::existing(c, &source_type, &original.id)?;
    let created = existing.is_none();
    let object = match existing {
        Some(id) => objects::get_with_defaults(c, actor, &id, true)?,
        None => {
            let receipt = objects::submit(
                tx,
                actor,
                Submit {
                    scope: request.scope.clone(),
                    crew_id: request.crew_id.clone(),
                    request_id: request.request_id.clone(),
                    change: request.change,
                },
            )?;
            db::record(
                tx.conn(),
                number(object_id(&receipt.object))?,
                &source_type,
                &original,
            )?;
            receipt.object
        }
    };
    let receipt = RecordReceipt {
        scope: request.scope,
        crew_id: request.crew_id,
        request_id: request.request_id,
        recorded_at: tx.now_ts(),
        source_message_id: original.id,
        created,
        object,
    };
    db::record_receipt(
        tx.conn(),
        &principal,
        number(object_id(&receipt.object))?,
        &digest,
        &receipt,
    )?;
    Ok(receipt)
}

pub fn lookup(
    c: &Connection,
    actor: Actor<'_>,
    origin: &Scope,
    crew: &str,
    request: &str,
) -> Result<Lookup> {
    if !crewd_proto::valid_remote_uuid(request)
        || !crewd_proto::valid_remote_uuid(&origin.restore_epoch)
    {
        return Err(invalid("requestId/restoreEpoch"));
    }
    let principal = actor.owner(c, crew, false)?;
    let current = objects::scope(c)?;
    if current.db_id != origin.db_id {
        return Err(forbidden());
    }
    let receipt = db::receipt(c, crew, &principal, request)?.map(|(_, receipt)| receipt);
    if let Some(saved) = &receipt {
        // A deleted source is readable only through the filtered snapshot.
        match source(c, actor, object_id(&saved.object))? {
            Source::Restricted | Source::None => return Err(forbidden()),
            _ => {}
        }
    }
    if current.restore_epoch != origin.restore_epoch {
        return Ok(Lookup::Historical {
            receipt: receipt.filter(|receipt| receipt.scope == *origin),
        });
    }
    Ok(match receipt {
        Some(receipt) if receipt.scope == *origin => Lookup::Known { receipt },
        Some(_) => Lookup::Historical { receipt: None },
        None => Lookup::NotFound,
    })
}
