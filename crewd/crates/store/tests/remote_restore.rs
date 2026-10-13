//! Store-facts acceptance; certificates below are inert witnesses, not TLS handshake evidence.
use crewd_proto::{RemoteGrantScope, RemoteGrantSummary, RemotePublicIdentity};
use crewd_store::{Store, backup, meta_get, remote};

#[test]
fn restoring_an_active_backup_does_not_revive_revoked_remote_trust() {
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Restore trust evidence: {}", root.display());
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    let db = data.join(crewd_store::DB_FILE);
    let store = Store::open_data_dir(&data).unwrap();
    assert_eq!(store.conn().query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 11);
    let identity = RemotePublicIdentity {
        authority_id: "11111111-1111-4111-8111-111111111111".into(),
        db_id: meta_get(store.conn(), "dbId").unwrap().unwrap(),
        certificate_der_base64: "inert-store-server-certificate".into(),
        spki_sha256: "a".repeat(64),
    };
    let grant = RemoteGrantSummary {
        id: "22222222-2222-4222-8222-222222222222".into(),
        scope: RemoteGrantScope::OwnerWorkspace,
        device_label: "Synthetic owner device".into(),
        client_spki_sha256: "b".repeat(64),
        revision: "1".into(),
        created_at: "2026-10-14T00:00:00Z".into(),
        revoked_at: None,
        revoke_note: None,
    };
    remote::bind_identity(store.conn(), &identity, "2026-10-14T00:00:00Z").unwrap();
    remote::insert_grant(store.conn(), &identity.authority_id, &grant, "inert-store-client-certificate").unwrap();
    assert!(remote::authenticated_grant(store.conn(), &identity.authority_id, &grant.client_spki_sha256).unwrap().is_some());
    let snapshot = root.join("old-active.db");
    backup::backup_db(&db, &snapshot).unwrap();
    let original_backup_bytes = std::fs::read(&snapshot).unwrap();
    assert!(remote::revoke_grant(store.conn(), &grant.id, &identity.authority_id, 1, "Device revoked", "2026-10-14T00:01:00Z").unwrap());
    assert!(remote::authenticated_grant(store.conn(), &identity.authority_id, &grant.client_spki_sha256).unwrap().is_none());
    drop(store); // No serve process or live writer; restore's exclusive-owner precondition holds.
    let report = backup::restore(&data, &snapshot).unwrap();
    assert!(report.replaced.unwrap().is_file());
    assert_eq!(std::fs::read(&snapshot).unwrap(), original_backup_bytes, "restore may invalidate only its staged copy, never rewrite the backup");
    let restored = Store::open_data_dir(&data).unwrap();
    assert_eq!(meta_get(restored.conn(), "dbId").unwrap().as_deref(), Some(identity.db_id.as_str()));
    let restored_identity = remote::identity(restored.conn()).unwrap().unwrap();
    assert_eq!(restored_identity.authority_id, identity.authority_id);
    assert_eq!(restored_identity.spki_sha256, identity.spki_sha256);
    assert_eq!(restored_identity.certificate_der_base64, identity.certificate_der_base64);
    assert!(remote::authenticated_grant(restored.conn(), &identity.authority_id, &grant.client_spki_sha256).unwrap().is_none(), "same-identity restore revived a previously revoked device grant");
}
