use super::*;
use std::os::unix::fs::MetadataExt;

type OriginalFiles = Vec<(String, Option<(Vec<u8>, u32, u64)>)>;
fn original_files(dir: &Path) -> OriginalFiles {
    [
        DB_FILE,
        ID_FILE,
        "crewd.db-wal",
        "crewd.db-shm",
        "crewd.db-journal",
    ]
    .into_iter()
    .map(|name| {
        let p = dir.join(name);
        let value = match std::fs::read(&p) {
            Ok(bytes) => {
                let m = std::fs::metadata(&p).unwrap();
                Some((bytes, m.mode(), m.ino()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => panic!("original census failed: {e}"),
        };
        (name.into(), value)
    })
    .collect()
}

#[test]
fn recovery_guard_current_database_and_committed_wal_use_private_copy_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_data_dir(dir.path()).unwrap();
    meta_set(store.conn(), "recoveryProbe", "durable-current-value").unwrap();
    assert!(
        std::fs::metadata(dir.path().join("crewd.db-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = original_files(dir.path());
    let guard = Store::recovery_input_guard(dir.path()).unwrap();
    assert!(guard.database_present());
    guard.recheck().unwrap();
    assert_eq!(
        before,
        original_files(dir.path()),
        "no original SQLite sidecar/bytes/mode/inode effect"
    );
    assert_eq!(
        meta_get(store.conn(), "recoveryProbe").unwrap().as_deref(),
        Some("durable-current-value")
    );
}

#[test]
fn recovery_guard_future_header_refuses_before_original_or_registry_effects() {
    let dir = tempfile::tempdir().unwrap();
    drop(Store::open_data_dir(dir.path()).unwrap());
    let p = dir.path().join(DB_FILE);
    let mut bytes = std::fs::read(&p).unwrap();
    bytes[60..64].copy_from_slice(&((schema::SCHEMA_VERSION + 1) as u32).to_be_bytes());
    std::fs::write(&p, bytes).unwrap();
    let before = original_files(dir.path());
    let error = Store::recovery_input_guard(dir.path())
        .err()
        .expect("future not admitted");
    assert!(error.to_string().contains("新"), "{error}");
    assert_eq!(before, original_files(dir.path()));
}

#[test]
fn recovery_guard_corrupt_btree_and_bad_wal_are_zero_touch_rejections() {
    for bad_wal in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::open_data_dir(dir.path()).unwrap());
        if bad_wal {
            std::fs::write(dir.path().join("crewd.db-wal"), [0u8; 64]).unwrap();
        } else {
            let p = dir.path().join(DB_FILE);
            let mut bytes = std::fs::read(&p).unwrap();
            bytes[100] = 0xff;
            std::fs::write(p, bytes).unwrap();
        }
        let before = original_files(dir.path());
        assert!(Store::recovery_input_guard(dir.path()).is_err());
        assert_eq!(
            before,
            original_files(dir.path()),
            "refuse exact original corruption/unknown WAL"
        );
    }
}

#[test]
fn recovery_guard_id_mismatch_and_changed_input_reject_without_promotion() {
    let dir = tempfile::tempdir().unwrap();
    drop(Store::open_data_dir(dir.path()).unwrap());
    let guard = Store::recovery_input_guard(dir.path()).unwrap();
    std::fs::write(dir.path().join(ID_FILE), "foreign-id\n").unwrap();
    assert!(guard.recheck().is_err());
    let before = original_files(dir.path());
    assert!(matches!(
        Store::recovery_input_guard(dir.path()),
        Err(StoreError::DbIdMismatch { .. })
    ));
    assert_eq!(before, original_files(dir.path()));
}
