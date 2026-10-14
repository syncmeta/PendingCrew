//! Security admission projection, never an execution/result authority.
use crate::{Result, StoreError};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedClient {
    pub id: String,
    pub db_id: String,
    pub mode: String,
    pub signing_identifier: String,
    pub cd_hash: String,
    pub team: Option<String>,
    pub requirement: String,
    pub created_at: String,
}
pub const V14_DDL: &str = r#"
CREATE TABLE local_client_trust(
 id TEXT PRIMARY KEY,
 db_id TEXT NOT NULL,
 mode TEXT NOT NULL CHECK(mode IN('exactCode','publisher')),
 signing_identifier TEXT NOT NULL,
 cd_hash TEXT NOT NULL,
 team TEXT,
 requirement TEXT NOT NULL,
 created_at TEXT NOT NULL,
 CHECK(length(requirement) BETWEEN 1 AND 2048),
 CHECK(mode='exactCode' OR team IS NOT NULL)
);
"#;
pub fn list(c: &Connection) -> Result<Vec<TrustedClient>> {
    let db = crate::meta_get(c, "dbId")?
        .ok_or_else(|| StoreError::Invalid("trust database identity missing".into()))?;
    let mut q=c.prepare("SELECT id,db_id,mode,signing_identifier,cd_hash,team,requirement,created_at FROM local_client_trust WHERE db_id=?1 ORDER BY id")?;
    Ok(q.query_map([db], |r| {
        Ok(TrustedClient {
            id: r.get(0)?,
            db_id: r.get(1)?,
            mode: r.get(2)?,
            signing_identifier: r.get(3)?,
            cd_hash: r.get(4)?,
            team: r.get(5)?,
            requirement: r.get(6)?,
            created_at: r.get(7)?,
        })
    })?
    .collect::<rusqlite::Result<_>>()?)
}
pub fn grant(c: &Connection, trust: &TrustedClient) -> Result<TrustedClient> {
    if c.is_autocommit() || crate::meta_get(c, "dbId")?.as_deref() != Some(&trust.db_id) {
        return Err(StoreError::Invalid(
            "local trust requires owning authority transaction".into(),
        ));
    }
    if let Some(existing) = list(c)?.into_iter().find(|existing| {
        existing.requirement == trust.requirement
            && existing.mode == trust.mode
            && existing.signing_identifier == trust.signing_identifier
    }) {
        return Ok(existing);
    }
    let count: i64 = c.query_row("SELECT COUNT(*) FROM local_client_trust", [], |r| r.get(0))?;
    if count >= 16 {
        return Err(StoreError::Invalid(
            "local trust capacity exhausted; explicitly revoke an old pin".into(),
        ));
    }
    c.execute("INSERT INTO local_client_trust(id,db_id,mode,signing_identifier,cd_hash,team,requirement,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![trust.id,trust.db_id,trust.mode,trust.signing_identifier,trust.cd_hash,trust.team,trust.requirement,trust.created_at])?;
    Ok(trust.clone())
}
pub fn revoke(c: &Connection, id: &str) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "local trust revoke requires owning transaction".into(),
        ));
    }
    c.execute("DELETE FROM local_client_trust WHERE id=?1", [id])?;
    Ok(())
}
pub fn restored(c: &Connection) -> Result<()> {
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='local_client_trust')",[],|r|r.get(0))?;
    if exists {
        c.execute("DELETE FROM local_client_trust", [])?;
    }
    Ok(())
}
