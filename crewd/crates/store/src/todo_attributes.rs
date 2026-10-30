//! Source-global facts and original mutation receipts. Transactions belong to Core.
use crate::{Result, StoreError};
use crewd_proto::*;
use rusqlite::{Connection, OptionalExtension, params};
pub fn get(c: &Connection, o: &ChannelWorkObject) -> Result<Option<TodoAttributes>> {
    let raw:Option<String>=c.query_row("SELECT body FROM todo_attribute_events WHERE object_kind=?1 AND object_id=?2 ORDER BY revision DESC LIMIT 1",params![crate::channel_work::kind(&o.kind),o.id],|r|r.get(0)).optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(StoreError::from))
        .transpose()
}
pub fn save(c: &Connection, a: &TodoAttributes, expected: i64, at: &str) -> Result<()> {
    let revision = a
        .revision
        .parse::<i64>()
        .map_err(|_| StoreError::Invalid("invalid todo revision".into()))?;
    if expected.checked_add(1) != Some(revision) {
        return Err(StoreError::Invalid("todo revision exhausted".into()));
    }
    let kind = crate::channel_work::kind(&a.object.kind);
    let owner = a.owner.as_ref().map(serde_json::to_string).transpose()?;
    let state = a.manual_state.map(|v| {
        if v == ManualTodoState::Done {
            "done"
        } else {
            "open"
        }
    });
    let origin = match a.origin {
        TodoOrigin::Requirement => "requirement",
        TodoOrigin::AgentAsk => "agentAsk",
        TodoOrigin::HumanTask => "humanTask",
    };
    let changed = if expected == 0 {
        c.execute("INSERT INTO todo_attributes(object_kind,object_id,crew_id,revision,origin,owner,manual_state)VALUES(?1,?2,?3,?4,?5,?6,?7)",params![kind,a.object.id,a.crew_id,revision,origin,owner,state])?
    } else {
        c.execute("UPDATE todo_attributes SET revision=?4,owner=?5,manual_state=?6 WHERE object_kind=?1 AND object_id=?2 AND revision=?3",params![kind,a.object.id,expected,revision,owner,state])?
    };
    if changed != 1 {
        return Err(StoreError::Invalid(
            "todo attribute CAS changed within transaction".into(),
        ));
    }
    c.execute("INSERT INTO todo_attribute_events(object_kind,object_id,crew_id,revision,body,at)VALUES(?1,?2,?3,?4,?5,?6)",params![kind,a.object.id,a.crew_id,revision,serde_json::to_string(a)?,at])?;
    Ok(())
}
#[derive(Debug)]
pub struct Mutation {
    pub operation: String,
    pub crew: String,
    pub channel: String,
    pub object_kind: String,
    pub object_id: String,
    pub digest: Vec<u8>,
    pub result: serde_json::Value,
}
pub fn request(c: &Connection, id: &str) -> Result<Option<Mutation>> {
    type RequestRow = (String, String, String, String, String, Vec<u8>, String);
    let row:Option<RequestRow>=c.query_row("SELECT operation,crew_id,channel_id,object_kind,object_id,payload_digest,result FROM resource_mutation_requests WHERE request_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    row.map(|(operation, crew, channel, kind, id, digest, result)| {
        Ok(Mutation {
            operation,
            crew,
            channel,
            object_kind: kind,
            object_id: id,
            digest,
            result: serde_json::from_str(&result)?,
        })
    })
    .transpose()
}
pub fn record(c: &Connection, id: &str, m: &Mutation, at: &str) -> Result<()> {
    c.execute("INSERT INTO resource_mutation_requests(request_id,operation,crew_id,channel_id,object_kind,object_id,payload_digest,result,at)VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,m.operation,m.crew,m.channel,m.object_kind,m.object_id,m.digest,m.result.to_string(),at])?;
    Ok(())
}
