//! Durable remote identity/grant facts. TLS and caller qualification belong to the server.
//! Callers perform mutation and its witness/audit in their single owning transaction.
use crate::{Result, StoreError};
use crewd_proto::{
    RemoteConfiguration, RemoteGrantScope, RemoteGrantSummary, RemotePublicIdentity,
    valid_remote_uuid, valid_spki_pin,
};
use rusqlite::{Connection, OptionalExtension, params};

pub fn identity(conn: &Connection) -> Result<Option<RemotePublicIdentity>> {
    Ok(conn.query_row("SELECT authority_id,db_id,certificate_der_base64,spki_sha256 FROM remote_authority WHERE singleton=1", [], |r| {
        let pin: String = r.get(3)?;
        if !valid_spki_pin(&pin) { return Err(rusqlite::Error::InvalidQuery); }
        Ok(RemotePublicIdentity { authority_id: r.get(0)?, db_id: r.get(1)?, certificate_der_base64: r.get(2)?, spki_sha256: pin })
    }).optional()?)
}

pub fn bind_identity(conn: &Connection, identity: &RemotePublicIdentity, at: &str) -> Result<()> {
    let db_id: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='dbId'", [], |r| r.get(0))
        .optional()?;
    if db_id.as_deref() != Some(&identity.db_id)
        || !valid_remote_uuid(&identity.authority_id)
        || !valid_spki_pin(&identity.spki_sha256)
        || identity.certificate_der_base64.is_empty()
        || identity.certificate_der_base64.len() > crewd_proto::MAX_REMOTE_CERTIFICATE_BYTES * 2
    {
        return Err(StoreError::Invalid("invalid remote public identity".into()));
    }
    conn.execute("INSERT INTO remote_authority(singleton,authority_id,db_id,certificate_der_base64,spki_sha256,created_at) VALUES(1,?1,?2,?3,?4,?5)", params![identity.authority_id, identity.db_id, identity.certificate_der_base64, identity.spki_sha256, at])?;
    Ok(())
}

pub fn configuration(conn: &Connection) -> Result<RemoteConfiguration> {
    Ok(conn.query_row(
        "SELECT revision,enabled,listen_host,port FROM remote_configuration WHERE singleton=1",
        [],
        |r| {
            let revision: i64 = r.get(0)?;
            if revision <= 0 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(RemoteConfiguration {
                revision: revision.to_string(),
                enabled: r.get(1)?,
                listen_host: r.get(2)?,
                port: r.get(3)?,
            })
        },
    )?)
}

const GRANT_FIELDS: &str =
    "id,device_label,client_spki_sha256,revision,created_at,revoked_at,revoke_note";
fn grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteGrantSummary> {
    let revision: i64 = r.get(3)?;
    let pin: String = r.get(2)?;
    if revision <= 0 || !valid_spki_pin(&pin) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(RemoteGrantSummary {
        id: r.get(0)?,
        scope: RemoteGrantScope::OwnerWorkspace,
        device_label: r.get(1)?,
        client_spki_sha256: pin,
        revision: revision.to_string(),
        created_at: r.get(4)?,
        revoked_at: r.get(5)?,
        revoke_note: r.get(6)?,
    })
}
pub fn grant(conn: &Connection, id: &str, authority: &str) -> Result<Option<RemoteGrantSummary>> {
    Ok(conn
        .query_row(
            &format!("SELECT {GRANT_FIELDS} FROM remote_grants WHERE id=?1 AND authority_id=?2"),
            params![id, authority],
            grant_row,
        )
        .optional()?)
}
pub fn grants(conn: &Connection, authority: &str) -> Result<Vec<RemoteGrantSummary>> {
    let mut q = conn.prepare(&format!("SELECT {GRANT_FIELDS} FROM remote_grants WHERE authority_id=?1 ORDER BY created_at,id LIMIT 257"))?;
    let rows = q
        .query_map([authority], grant_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > crewd_proto::MAX_REMOTE_GRANTS {
        return Err(StoreError::Invalid("remote grant limit exceeded".into()));
    }
    Ok(rows)
}

/// The pin originates from the actual TLS certificate, not an RPC field.
/// Return only currently active grants under this exact server authority.
pub fn authenticated_grant(
    conn: &Connection,
    authority: &str,
    tls_pin: &str,
) -> Result<Option<(RemoteGrantScope, RemoteGrantSummary, String)>> {
    if !valid_spki_pin(tls_pin) {
        return Ok(None);
    }
    Ok(conn.query_row(&format!("SELECT {GRANT_FIELDS},client_certificate_der_base64 FROM remote_grants WHERE authority_id=?1 AND client_spki_sha256=?2 AND revoked_at IS NULL"), params![authority, tls_pin], |r| Ok((grant_row(r)?, r.get(7)?))).optional()?)
}

#[derive(Debug, Clone)]
pub struct RequestWitness {
    pub method: String,
    pub input_json: String,
    pub result_json: String,
}
pub fn request_witness(
    conn: &Connection,
    request_id: &str,
    authority: &str,
) -> Result<Option<RequestWitness>> {
    Ok(conn.query_row("SELECT method,input_json,result_json FROM remote_requests WHERE request_id=?1 AND authority_id=?2", params![request_id, authority], |r| Ok(RequestWitness { method: r.get(0)?, input_json: r.get(1)?, result_json: r.get(2)? })).optional()?)
}
pub fn record_request(
    conn: &Connection,
    authority: &str,
    request_id: &str,
    method: &str,
    input_json: &str,
    result_json: &str,
    at: &str,
) -> Result<()> {
    conn.execute("INSERT INTO remote_requests(request_id,authority_id,method,input_json,result_json,created_at) VALUES(?1,?2,?3,?4,?5,?6)", params![request_id, authority, method, input_json, result_json, at])?;
    Ok(())
}
pub fn audit(
    conn: &Connection,
    authority: &str,
    grant: Option<&str>,
    action: &str,
    request: Option<&str>,
    at: &str,
    finite_facts_json: &str,
) -> Result<()> {
    conn.execute("INSERT INTO remote_audit(authority_id,grant_id,action,request_id,at,facts_json) VALUES(?1,?2,?3,?4,?5,?6)", params![authority, grant, action, request, at, finite_facts_json])?;
    Ok(())
}
