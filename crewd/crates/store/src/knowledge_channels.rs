//! Core references only: knowledge content remains in its original SQLite namespace.
use crate::{Result, StoreError};
use crewd_proto::*;
use rusqlite::{Connection, params};
pub fn key(s: &ChannelKnowledgeReference) -> Result<String> {
    Ok(serde_json::to_string(&(
        &s.context.authority_id,
        &s.context.db_id,
        &s.context.restore_epoch,
        &s.context.crew_id,
        &s.reference.namespace,
        &s.reference.kind,
        &s.reference.id,
    ))?)
}
pub fn rows(c: &Connection, ch: &str) -> Result<Vec<ChannelKnowledgeReference>> {
    let mut q=c.prepare("SELECT source FROM channel_knowledge_links WHERE channel_id=?1 ORDER BY source_key LIMIT 4097")?;
    let rows = q
        .query_map([ch], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 4096 {
        return Err(StoreError::Invalid("knowledge group bound exceeded".into()));
    }
    rows.into_iter()
        .map(|s| serde_json::from_str(&s).map_err(StoreError::from))
        .collect()
}
pub fn linked(c: &Connection, ch: &str, s: &ChannelKnowledgeReference) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM channel_knowledge_links WHERE channel_id=?1 AND source_key=?2)",params![ch,key(s)?],|r|r.get(0))?)
}
pub fn change(
    c: &Connection,
    ch: &str,
    s: &ChannelKnowledgeReference,
    expected: i64,
    link: bool,
    actor: &ResourceActor,
    at: &str,
) -> Result<i64> {
    c.execute(
        "INSERT OR IGNORE INTO channel_work_scopes(channel_id,revision)VALUES(?1,0)",
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
            "channel reference CAS changed in transaction".into(),
        ));
    }
    if link {
        c.execute("INSERT INTO channel_knowledge_links(channel_id,source_key,source_crew_id,source)VALUES(?1,?2,?3,?4)",params![ch,key(s)?,s.context.crew_id,serde_json::to_string(s)?])?;
    } else {
        c.execute(
            "DELETE FROM channel_knowledge_links WHERE channel_id=?1 AND source_key=?2",
            params![ch, key(s)?],
        )?;
    }
    c.execute(
        "INSERT INTO channel_work_events(channel_id,revision,actor,body,at)VALUES(?1,?2,?3,?4,?5)",
        params![
            ch,
            next,
            serde_json::to_string(actor)?,
            serde_json::json!({"knowledgeSource":s,"linked":link}).to_string(),
            at
        ],
    )?;
    Ok(next)
}
