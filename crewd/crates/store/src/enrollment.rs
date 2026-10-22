//! Enrollment format1 phase records. Same SQLite transaction as original grant facts.
//! No token plaintext, replacement authority, new content source or generic intent journal.
use crate::{Result, StoreError, meta_get, meta_set};
use crewd_proto::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
const PREFIX: &str = "remote.enrollment.v1.";
const MAX_RECORDS: usize = 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentRecord {
    pub format: u32,
    pub enrollment_id: String,
    pub create_request_id: String,
    pub create_digest: String,
    pub authority_id: String,
    pub db_id: String,
    pub token_hash: String,
    pub expires_unix: i64,
    pub computer_label: String,
    pub endpoint: RemoteEndpoint,
    pub reservation: Option<Reservation>,
    pub committed: Option<Committed>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reservation {
    pub request_id: String,
    pub input_digest: String,
    pub client_certificate_der_base64: String,
    pub client_spki_sha256: String,
    pub device_label: String,
    pub invitation: RemoteSignedEnvelope,
    pub invitation_digest: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Committed {
    pub input_digest: String,
    pub result: RemoteAcceptResult,
}
fn invalid() -> StoreError {
    StoreError::Invalid("invalid or changed original enrollment".into())
}
fn key(id: &str) -> Result<String> {
    if !valid_remote_uuid(id) {
        return Err(invalid());
    }
    Ok(format!("{PREFIX}{id}"))
}
fn transaction(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(StoreError::Invalid(
            "enrollment phase requires original transaction".into(),
        ));
    }
    Ok(())
}
pub fn get(conn: &Connection, authority: &str, id: &str) -> Result<Option<EnrollmentRecord>> {
    let Some(text) = meta_get(conn, &key(id)?)? else {
        return Ok(None);
    };
    let record: EnrollmentRecord = serde_json::from_str(&text).map_err(|_| invalid())?;
    if record.format != ENROLLMENT_FORMAT
        || record.enrollment_id != id
        || record.authority_id != authority
        || meta_get(conn, "dbId")?.as_deref() != Some(record.db_id.as_str())
    {
        return Err(invalid());
    }
    Ok(Some(record))
}
fn save(conn: &Connection, record: &EnrollmentRecord) -> Result<()> {
    meta_set(
        conn,
        &key(&record.enrollment_id)?,
        &serde_json::to_string(record).map_err(|_| invalid())?,
    )
}
pub fn create(conn: &Connection, record: &EnrollmentRecord, now: i64) -> Result<EnrollmentRecord> {
    transaction(conn)?;
    if record.format != ENROLLMENT_FORMAT
        || !valid_remote_uuid(&record.create_request_id)
        || !valid_spki_pin(&record.create_digest)
        || !valid_spki_pin(&record.token_hash)
        || record.expires_unix <= now
        || record.expires_unix - now > ENROLLMENT_TTL_SECONDS
        || !record.endpoint.is_valid()
        || record.reservation.is_some()
        || record.committed.is_some()
        || record.computer_label.trim().is_empty()
        || record.computer_label.len() > MAX_REMOTE_LABEL_BYTES
        || meta_get(conn, "dbId")?.as_deref() != Some(record.db_id.as_str())
    {
        return Err(invalid());
    }
    if let Some(old) = get(conn, &record.authority_id, &record.enrollment_id)? {
        if old.create_request_id != record.create_request_id
            || old.create_digest != record.create_digest
        {
            return Err(invalid());
        }
        return Ok(old);
    }
    let mut statement =
        conn.prepare("SELECT value FROM meta WHERE key GLOB 'remote.enrollment.v1.*'")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut total = 0usize;
    let mut pending = 0usize;
    for row in rows {
        let item: EnrollmentRecord = serde_json::from_str(&row?).map_err(|_| invalid())?;
        if item.format != ENROLLMENT_FORMAT {
            return Err(invalid());
        }
        total += 1;
        if item.create_request_id == record.create_request_id {
            return Err(invalid());
        }
        if item.committed.is_none() && item.expires_unix > now {
            pending += 1
        }
    }
    if total >= MAX_RECORDS || pending >= MAX_REMOTE_PENDING_INVITATIONS {
        return Err(StoreError::Invalid("enrollment capacity reached".into()));
    }
    let identity = crate::remote::identity(conn)?.ok_or_else(invalid)?;
    if identity.authority_id != record.authority_id || identity.db_id != record.db_id {
        return Err(invalid());
    }
    save(conn, record)?;
    Ok(record.clone())
}
pub fn reserve(
    conn: &Connection,
    authority: &str,
    id: &str,
    next: Reservation,
    now: i64,
) -> Result<EnrollmentRecord> {
    transaction(conn)?;
    let mut record = get(conn, authority, id)?.ok_or_else(invalid)?;
    if let Some(old) = &record.reservation {
        if old.request_id != next.request_id || old.input_digest != next.input_digest {
            return Err(invalid());
        }
        return Ok(record);
    }
    if record.expires_unix <= now
        || record.committed.is_some()
        || !valid_remote_uuid(&next.request_id)
        || !valid_spki_pin(&next.input_digest)
        || !valid_spki_pin(&next.client_spki_sha256)
        || !valid_spki_pin(&next.invitation_digest)
        || !next.invitation.is_bounded()
        || next.client_certificate_der_base64.is_empty()
        || next.client_certificate_der_base64.len() > MAX_REMOTE_CERTIFICATE_BYTES * 2
        || next.device_label.trim().is_empty()
        || next.device_label.len() > MAX_REMOTE_LABEL_BYTES
    {
        return Err(invalid());
    }
    reservation_invitation(conn, &record, &next)?;
    record.reservation = Some(next);
    save(conn, &record)?;
    Ok(record)
}
/// Validate the one B pending authority; no legacy approval row exists yet.
/// Server additionally verifies the original signature before creating the legacy
/// invitation inside the same grant/consume/witness/committed transaction.
pub fn reservation_invitation(
    conn: &Connection,
    record: &EnrollmentRecord,
    reserved: &Reservation,
) -> Result<RemoteInvitation> {
    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(&reserved.invitation.payload)
        .map_err(|_| invalid())?;
    let hash: String = ring::digest::digest(&ring::digest::SHA256, &payload)
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    if hash != reserved.invitation_digest || !reserved.invitation.is_bounded() {
        return Err(invalid());
    }
    let invitation: RemoteInvitation = serde_json::from_slice(&payload).map_err(|_| invalid())?;
    let server = crate::remote::identity(conn)?.ok_or_else(invalid)?;
    let expiry =
        chrono::DateTime::parse_from_rfc3339(&invitation.expires_at).map_err(|_| invalid())?;
    if invitation.invitation_id != record.enrollment_id
        || invitation.server != server
        || server.authority_id != record.authority_id
        || server.db_id != record.db_id
        || invitation.endpoint != record.endpoint
        || invitation.computer_label != record.computer_label
        || expiry.timestamp() != record.expires_unix
        || invitation.challenge.is_empty()
        || invitation.challenge.len() > 128
    {
        return Err(invalid());
    }
    Ok(invitation)
}
pub fn commit(
    conn: &Connection,
    authority: &str,
    id: &str,
    next: Committed,
) -> Result<EnrollmentRecord> {
    transaction(conn)?;
    let mut record = get(conn, authority, id)?.ok_or_else(invalid)?;
    let original = record.reservation.as_ref().ok_or_else(invalid)?;
    if let Some(old) = &record.committed {
        if old.input_digest != next.input_digest || old.result.request_id != next.result.request_id
        {
            return Err(invalid());
        }
        return Ok(record);
    }
    if !valid_spki_pin(&next.input_digest)
        || next.result.request_id != original.request_id
        || next.result.grant.client_spki_sha256 != original.client_spki_sha256
        || !next.result.receipt.is_bounded()
    {
        return Err(invalid());
    }
    let grant =
        crate::remote::grant(conn, &next.result.grant.id, authority)?.ok_or_else(invalid)?;
    let invitation = crate::remote::invitation(conn, id, authority)?.ok_or_else(invalid)?;
    let witness = crate::remote::request_witness(conn, &original.request_id, authority)?
        .ok_or_else(invalid)?;
    let witnessed: RemoteAcceptResult =
        serde_json::from_str(&witness.result_json).map_err(|_| invalid())?;
    if grant != next.result.grant
        || invitation.consumed_at.is_none()
        || witness.method != "remote.accept"
        || witnessed != next.result
    {
        return Err(invalid());
    }
    record.committed = Some(next);
    save(conn, &record)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    fn fixture(store: &Store) -> EnrollmentRecord {
        let authority = "11111111-1111-4111-8111-111111111111".to_string();
        let db_id = meta_get(store.conn(), "dbId").unwrap().unwrap();
        crate::remote::bind_identity(
            store.conn(),
            &RemotePublicIdentity {
                authority_id: authority.clone(),
                db_id: db_id.clone(),
                certificate_der_base64: "public-certificate-placeholder".into(),
                spki_sha256: "a".repeat(64),
            },
            "test",
        )
        .unwrap();
        EnrollmentRecord {
            format: 1,
            enrollment_id: "22222222-2222-4222-8222-222222222222".into(),
            create_request_id: "33333333-3333-4333-8333-333333333333".into(),
            create_digest: "b".repeat(64),
            authority_id: authority,
            db_id,
            token_hash: "c".repeat(64),
            expires_unix: 1300,
            computer_label: "Private host".into(),
            endpoint: RemoteEndpoint {
                host: "127.0.0.1".into(),
                port: 49381,
            },
            reservation: None,
            committed: None,
        }
    }
    #[test]
    fn enrollment_creation_is_transactional_reopens_and_rejects_changed_original_uuid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("core.db");
        let mut store = Store::open(&path).unwrap();
        let record = fixture(&store);
        assert!(create(store.conn(), &record, 1000).is_err());
        let failure: Result<()> = store.with_tx(|tx| {
            create(tx, &record, 1000)?;
            Err(invalid())
        });
        assert!(failure.is_err());
        assert!(
            get(store.conn(), &record.authority_id, &record.enrollment_id)
                .unwrap()
                .is_none()
        );
        store.with_tx(|tx| create(tx, &record, 1000)).unwrap();
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        let found = get(reopened.conn(), &record.authority_id, &record.enrollment_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.token_hash, record.token_hash);
        assert!(found.reservation.is_none());
        // Original result survives expiry; this is not a new token or activation.
        // Lookup does not call create: directly read the existing immutable record.
        assert!(
            get(
                reopened.conn(),
                "44444444-4444-4444-8444-444444444444",
                &record.enrollment_id
            )
            .is_err()
        );
        let mut changed = record.clone();
        changed.create_digest = "d".repeat(64);
        assert!(reopened.with_tx(|tx| create(tx, &changed, 1000)).is_err());
        let mut second = record.clone();
        second.enrollment_id = "55555555-5555-4555-8555-555555555555".into();
        assert!(reopened.with_tx(|tx| create(tx, &second, 1000)).is_err());
        assert_eq!(
            get(reopened.conn(), &record.authority_id, &record.enrollment_id)
                .unwrap()
                .unwrap()
                .create_digest,
            record.create_digest
        );
    }
    #[test]
    fn enrollment_expiry_and_pending_budget_preserve_original_records() {
        let mut store = Store::open_in_memory().unwrap();
        let base = fixture(&store);
        let mut too_long = base.clone();
        too_long.expires_unix = 1301;
        assert!(store.with_tx(|tx| create(tx, &too_long, 1000)).is_err());
        let mut expired = base.clone();
        expired.expires_unix = 1000;
        assert!(store.with_tx(|tx| create(tx, &expired, 1000)).is_err());
        for n in 0..MAX_REMOTE_PENDING_INVITATIONS {
            let mut record = base.clone();
            record.enrollment_id = format!("22222222-2222-4222-8222-{n:012x}");
            record.create_request_id = format!("33333333-3333-4333-8333-{n:012x}");
            store.with_tx(|tx| create(tx, &record, 1000)).unwrap();
        }
        let mut overflow = base.clone();
        overflow.enrollment_id = "44444444-4444-4444-8444-444444444444".into();
        overflow.create_request_id = "55555555-5555-4555-8555-555555555555".into();
        assert!(store.with_tx(|tx| create(tx, &overflow, 1000)).is_err());
        let first = "22222222-2222-4222-8222-000000000000";
        let before = meta_get(store.conn(), &key(first).unwrap())
            .unwrap()
            .unwrap();
        // Expiry frees pending capacity without erasing the prior result/key.
        overflow.expires_unix = 1600;
        store.with_tx(|tx| create(tx, &overflow, 1300)).unwrap();
        assert_eq!(
            meta_get(store.conn(), &key(first).unwrap())
                .unwrap()
                .unwrap(),
            before
        );
        let old = get(store.conn(), &base.authority_id, first)
            .unwrap()
            .unwrap();
        assert!(old.reservation.is_none());
        assert_eq!(old.expires_unix, 1300);
        let rows: i64 = store
            .conn()
            .query_row(
                "SELECT count(*) FROM meta WHERE key GLOB 'remote.enrollment.v1.*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 33);
    }
    #[test]
    fn enrollment_commit_cannot_invent_grant_without_original_phase_and_facts() {
        let mut store = Store::open_in_memory().unwrap();
        let record = fixture(&store);
        store.with_tx(|tx| create(tx, &record, 1000)).unwrap();
        let next = Reservation {
            request_id: "66666666-6666-4666-8666-666666666666".into(),
            input_digest: "a".repeat(64),
            client_certificate_der_base64: "public-only".into(),
            client_spki_sha256: "b".repeat(64),
            device_label: "Device".into(),
            invitation: RemoteSignedEnvelope {
                format: 1,
                payload: "YQ==".into(),
                signature: "YQ==".into(),
            },
            invitation_digest: "c".repeat(64),
        };
        assert!(
            store
                .with_tx(|tx| reserve(tx, &record.authority_id, &record.enrollment_id, next, 1000))
                .is_err()
        );
        assert!(
            get(store.conn(), &record.authority_id, &record.enrollment_id)
                .unwrap()
                .unwrap()
                .reservation
                .is_none()
        );
        // Corrupt independent format cannot be silently cleared or reinitialized.
        store
            .with_tx(|tx| {
                meta_set(tx, &key(&record.enrollment_id)?, "{\"format\":2}")?;
                Ok(())
            })
            .unwrap();
        assert!(get(store.conn(), &record.authority_id, &record.enrollment_id).is_err());
    }
}
