//! Reference-only channel scopes and immutable work-creation identity witnesses.
use crate::{Result, StoreError};
use crewd_proto::{ChannelWorkKind, ChannelWorkObject, ResourceActor};
use rusqlite::{Connection, OptionalExtension, params};
pub fn kind(k: &ChannelWorkKind) -> &'static str {
    match k {
        ChannelWorkKind::Requirement => "requirement",
        ChannelWorkKind::Work => "work",
        ChannelWorkKind::HumanTask => "humanTask",
    }
}
pub fn revision(c: &Connection, ch: &str) -> Result<i64> {
    let n = c
        .query_row(
            "SELECT revision FROM channel_work_scopes WHERE channel_id=?1",
            [ch],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    if n < 0 {
        return Err(StoreError::Invalid(
            "negative channel reference revision".into(),
        ));
    }
    Ok(n)
}
pub fn linked(c: &Connection, ch: &str, o: &ChannelWorkObject) -> Result<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM channel_work_links WHERE channel_id=?1 AND object_kind=?2 AND object_id=?3",
        params![ch, kind(&o.kind), o.id],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}
pub fn rows(c: &Connection, ch: &str) -> Result<Vec<ChannelWorkObject>> {
    let mut q=c.prepare("SELECT object_kind,object_id FROM channel_work_links WHERE channel_id=?1 ORDER BY object_kind,object_id LIMIT 4097")?;
    let rows = q
        .query_map([ch], |r| {
            let s: String = r.get(0)?;
            Ok(ChannelWorkObject {
                kind: match s.as_str() {
                    "requirement" => ChannelWorkKind::Requirement,
                    "work" => ChannelWorkKind::Work,
                    "humanTask" => ChannelWorkKind::HumanTask,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                },
                id: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 4096 {
        return Err(StoreError::Invalid(
            "channel reference limit violated".into(),
        ));
    }
    Ok(rows)
}
pub struct LinkChange<'a> {
    pub object: &'a ChannelWorkObject,
    pub source_crew: &'a str,
    pub linked: bool,
    pub actor: &'a ResourceActor,
    pub at: &'a str,
}
pub fn change(c: &Connection, ch: &str, expected: i64, change: LinkChange<'_>) -> Result<i64> {
    let LinkChange {
        object: o,
        source_crew: crew,
        linked: link,
        actor,
        at,
    } = change;
    c.execute(
        "INSERT OR IGNORE INTO channel_work_scopes(channel_id,revision) VALUES(?1,0)",
        [ch],
    )?;
    let next = expected
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid("channel revision exhausted".into()))?;
    if c.execute(
        "UPDATE channel_work_scopes SET revision=?1 WHERE channel_id=?2 AND revision=?3",
        params![next, ch, expected],
    )? != 1
    {
        return Err(StoreError::Invalid(
            "channel reference CAS changed within transaction".into(),
        ));
    }
    if link {
        c.execute("INSERT INTO channel_work_links(channel_id,object_kind,object_id,source_crew_id) VALUES(?1,?2,?3,?4)",params![ch,kind(&o.kind),o.id,crew])?;
    } else {
        c.execute("DELETE FROM channel_work_links WHERE channel_id=?1 AND object_kind=?2 AND object_id=?3",params![ch,kind(&o.kind),o.id])?;
    }
    c.execute(
        "INSERT INTO channel_work_events(channel_id,revision,actor,body,at) VALUES(?1,?2,?3,?4,?5)",
        params![
            ch,
            next,
            serde_json::to_string(actor)?,
            serde_json::json!({"object":o,"linked":link}).to_string(),
            at
        ],
    )?;
    Ok(next)
}
#[derive(Debug)]
pub struct CreationWitness {
    pub work_id: String,
    pub crew_id: String,
    pub creator_key: String,
    pub source_channel_id: Option<String>,
    pub digest: Vec<u8>,
}
pub fn creation(c: &Connection, request: &str) -> Result<Option<CreationWitness>> {
    Ok(c.query_row("SELECT work_id,crew_id,creator_key,source_channel_id,payload_digest FROM work_creation_requests WHERE request_id=?1",[request],|r|Ok(CreationWitness{work_id:r.get(0)?,crew_id:r.get(1)?,creator_key:r.get(2)?,source_channel_id:r.get(3)?,digest:r.get(4)?})).optional()?)
}
pub fn record_creation(c: &Connection, request: &str, w: &CreationWitness) -> Result<()> {
    c.execute("INSERT INTO work_creation_requests(request_id,work_id,crew_id,creator_key,source_channel_id,payload_digest) VALUES(?1,?2,?3,?4,?5,?6)",params![request,w.work_id,w.crew_id,w.creator_key,w.source_channel_id,w.digest])?;
    Ok(())
}
