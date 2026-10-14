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
    "id,device_label,client_spki_sha256,revision,created_at,revoked_at,revoke_note,scope";
fn grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteGrantSummary> {
    let revision: i64 = r.get(3)?;
    let pin: String = r.get(2)?;
    let scope: String = r.get(7)?;
    if revision <= 0 || !valid_spki_pin(&pin) || scope != "owner_workspace" {
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
) -> Result<Option<(RemoteGrantSummary, String)>> {
    if !valid_spki_pin(tls_pin) {
        return Ok(None);
    }
    Ok(conn.query_row(&format!("SELECT {GRANT_FIELDS},client_certificate_der_base64 FROM remote_grants WHERE authority_id=?1 AND client_spki_sha256=?2 AND revoked_at IS NULL"), params![authority, tls_pin], |r| Ok((grant_row(r)?, r.get(8)?))).optional()?)
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

/// Immutable invitation witness; no private signing material is stored here.
#[derive(Debug, Clone)]
pub struct StoredInvitation {
    pub challenge: String,
    pub envelope_json: String,
    pub expires_unix: i64,
    pub consumed_at: Option<String>,
}
pub fn invitation(
    conn: &Connection,
    id: &str,
    authority: &str,
) -> Result<Option<StoredInvitation>> {
    Ok(conn.query_row("SELECT challenge,envelope_json,expires_unix,consumed_at FROM remote_invitations WHERE id=?1 AND authority_id=?2", params![id, authority], |r| Ok(StoredInvitation { challenge: r.get(0)?, envelope_json: r.get(1)?, expires_unix: r.get(2)?, consumed_at: r.get(3)? })).optional()?)
}
pub struct NewInvitation<'a> {
    pub id: &'a str,
    pub authority: &'a str,
    pub challenge: &'a str,
    pub envelope_json: &'a str,
    pub expires_unix: i64,
    pub now_unix: i64,
    pub at: &'a str,
}
pub fn issue_invitation(conn: &Connection, input: NewInvitation<'_>) -> Result<()> {
    let NewInvitation {
        id,
        authority,
        challenge,
        envelope_json,
        expires_unix,
        now_unix,
        at,
    } = input;
    if !valid_remote_uuid(id)
        || challenge.len() > 128
        || challenge.is_empty()
        || envelope_json.len() > crewd_proto::MAX_REMOTE_ENVELOPE_BYTES * 2
        || expires_unix <= now_unix
        || expires_unix.saturating_sub(now_unix) > 300
    {
        return Err(StoreError::Invalid("invalid remote invitation".into()));
    }
    let pending: i64 = conn.query_row("SELECT COUNT(*) FROM remote_invitations WHERE authority_id=?1 AND consumed_at IS NULL AND expires_unix>?2", params![authority, now_unix], |r| r.get(0))?;
    if pending >= crewd_proto::MAX_REMOTE_PENDING_INVITATIONS as i64 {
        return Err(StoreError::Invalid(
            "remote invitation limit exceeded".into(),
        ));
    }
    conn.execute("INSERT INTO remote_invitations(id,authority_id,challenge,envelope_json,expires_unix,created_at) VALUES(?1,?2,?3,?4,?5,?6)", params![id, authority, challenge, envelope_json, expires_unix, at])?;
    Ok(())
}
pub fn consume_invitation(
    conn: &Connection,
    id: &str,
    authority: &str,
    now_unix: i64,
    at: &str,
) -> Result<bool> {
    Ok(conn.execute("UPDATE remote_invitations SET consumed_at=?1 WHERE id=?2 AND authority_id=?3 AND consumed_at IS NULL AND expires_unix>?4", params![at, id, authority, now_unix])? == 1)
}

pub fn insert_grant(
    conn: &Connection,
    authority: &str,
    grant: &RemoteGrantSummary,
    certificate_base64: &str,
) -> Result<()> {
    if !valid_remote_uuid(&grant.id)
        || !valid_spki_pin(&grant.client_spki_sha256)
        || grant.revision != "1"
        || grant.revoked_at.is_some()
        || grant.revoke_note.is_some()
        || grant.device_label.trim().is_empty()
        || grant.device_label.len() > crewd_proto::MAX_REMOTE_LABEL_BYTES
        || certificate_base64.is_empty()
        || certificate_base64.len() > crewd_proto::MAX_REMOTE_CERTIFICATE_BYTES * 2
    {
        return Err(StoreError::Invalid("invalid remote grant".into()));
    }
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM remote_grants WHERE authority_id=?1",
        [authority],
        |r| r.get(0),
    )?;
    if total >= crewd_proto::MAX_REMOTE_GRANTS as i64 {
        return Err(StoreError::Invalid("remote grant limit exceeded".into()));
    }
    conn.execute("INSERT INTO remote_grants(id,authority_id,device_label,client_spki_sha256,client_certificate_der_base64,revision,created_at) VALUES(?1,?2,?3,?4,?5,1,?6)", params![grant.id, authority, grant.device_label, grant.client_spki_sha256, certificate_base64, grant.created_at])?;
    Ok(())
}

/// Compare-and-set a permanent tombstone. No API reactivates an existing grant.
pub fn revoke_grant(
    conn: &Connection,
    id: &str,
    authority: &str,
    expected_revision: i64,
    note: &str,
    at: &str,
) -> Result<bool> {
    if expected_revision <= 0
        || expected_revision == i64::MAX
        || note.trim().is_empty()
        || note.len() > 4096
    {
        return Err(StoreError::Invalid(
            "invalid remote revoke parameters".into(),
        ));
    }
    Ok(conn.execute("UPDATE remote_grants SET revision=revision+1,revoked_at=?1,revoke_note=?2 WHERE id=?3 AND authority_id=?4 AND revision=?5 AND revoked_at IS NULL", params![at, note, id, authority, expected_revision])? == 1)
}

pub fn configure(
    conn: &Connection,
    expected_revision: i64,
    enabled: bool,
    host: &str,
    port: u16,
) -> Result<bool> {
    let endpoint = crewd_proto::RemoteEndpoint {
        host: host.into(),
        port,
    };
    if expected_revision <= 0 || expected_revision == i64::MAX || !endpoint.is_valid() {
        return Err(StoreError::Invalid("invalid remote configuration".into()));
    }
    Ok(conn.execute("UPDATE remote_configuration SET revision=revision+1,enabled=?1,listen_host=?2,port=?3 WHERE singleton=1 AND revision=?4", params![enabled, host, port, expected_revision])? == 1)
}

/// Re-evaluate the authoritative binding and grant for an already authenticated
/// TLS principal. Only actual server state supplies these expected identities.
/// The runtime/listener generation check remains in the owning server adapter.
pub fn live_grant(
    conn: &Connection,
    authority: &str,
    db_id: &str,
    server_pin: &str,
    client_pin: &str,
    grant_id: &str,
    configuration_revision: &str,
) -> Result<Option<RemoteGrantSummary>> {
    let identity = identity(conn)?;
    let configuration = configuration(conn)?;
    if identity.as_ref().is_none_or(|i| {
        i.authority_id != authority || i.db_id != db_id || i.spki_sha256 != server_pin
    }) || !configuration.enabled
        || configuration.revision != configuration_revision
    {
        return Ok(None);
    }
    let actual_db: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='dbId'", [], |r| r.get(0))
        .optional()?;
    if actual_db.as_deref() != Some(db_id) {
        return Ok(None);
    }
    Ok(grant(conn, grant_id, authority)?
        .filter(|g| g.revoked_at.is_none() && g.client_spki_sha256 == client_pin))
}

/// Work replacement cannot roll back device revocation. Execute this inside the
/// owning restore staging transaction or legacy-import work transaction. Source
/// backups and unrelated work/event/clock/revision facts remain unchanged.
pub fn invalidate_for_work_replacement(conn: &Connection, reason: &str, at: &str) -> Result<()> {
    if !matches!(reason, "work_restore" | "work_import") {
        return Err(StoreError::Invalid(
            "invalid remote trust invalidation reason".into(),
        ));
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='remote_grants')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(());
    } // Pre-M6 work snapshots contain no remote trust.
    let overflow: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM remote_grants WHERE revoked_at IS NULL AND revision>=9223372036854775807) OR EXISTS(SELECT 1 FROM remote_configuration WHERE revision>=9223372036854775807)", [], |r| r.get(0))?;
    if overflow {
        return Err(StoreError::Invalid(
            "remote trust revision exhausted".into(),
        ));
    }
    conn.execute(
        "UPDATE remote_configuration SET enabled=0,revision=revision+1 WHERE singleton=1",
        [],
    )?;
    conn.execute("UPDATE remote_grants SET revision=revision+1,revoked_at=?1,revoke_note='工作数据已替换，请重新配对' WHERE revoked_at IS NULL", [at])?;
    conn.execute(
        "UPDATE remote_invitations SET consumed_at=?1 WHERE consumed_at IS NULL",
        [at],
    )?;
    if let Some(identity) = identity(conn)? {
        audit(
            conn,
            &identity.authority_id,
            None,
            reason,
            None,
            at,
            r#"{"remoteTrustInvalidated":true}"#,
        )?;
    }
    Ok(())
}
