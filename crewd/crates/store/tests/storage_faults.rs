//! 存储故障的分类（S26）+ 故障注入（S30）：磁盘满、只读、不是数据库的文件。合成数据、临时目录。
use crewd_proto::Crew;
use crewd_store::{StorageKind, Store, StoreError, crews};
use std::os::unix::fs::PermissionsExt;

fn crew(id: &str, title: String) -> Crew {
    Crew {
        id: id.into(),
        title,
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: "2026-10-01T00:00:00.000Z".into(),
        ..Default::default()
    }
}

#[test]
fn disk_full_is_classified_and_reads_keep_working() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("crewd.db")).unwrap();
    crews::upsert(s.conn(), &crew("a", "示例".into()), None).unwrap();
    // 把库钉在当前大小：再写几页就是 SQLITE_FULL（和真磁盘满同一个错误码）。
    let pages: i64 = s
        .conn()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    s.conn()
        .execute_batch(&format!("PRAGMA max_page_count={pages}"))
        .unwrap();
    let big = "x".repeat(64 * 1024);
    let err = (0..50)
        .find_map(|i| crews::upsert(s.conn(), &crew(&format!("c{i}"), big.clone()), None).err())
        .expect("写满之前应当报错");
    assert!(
        matches!(
            err,
            StoreError::Storage {
                kind: StorageKind::DiskFull,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(err.storage_kind(), Some(StorageKind::DiskFull));
    // 写失败不连累读。
    assert!(!crews::list(s.conn()).unwrap().is_empty());
}

#[test]
fn a_file_that_is_not_a_database_is_classified_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("crewd.db");
    std::fs::write(&p, vec![0x5au8; 8192]).unwrap();
    let err = Store::open(&p).err().expect("不是库的文件必须打不开");
    assert_eq!(err.storage_kind(), Some(StorageKind::Corrupt), "{err:?}");
}

#[test]
fn a_read_only_database_file_is_classified_as_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("crewd.db");
    drop(Store::open(&p).unwrap());
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
    let err = match Store::open(&p) {
        Err(e) => e,
        Ok(s) => crews::upsert(s.conn(), &crew("a", "示例".into()), None).unwrap_err(),
    };
    assert_eq!(err.storage_kind(), Some(StorageKind::ReadOnly), "{err:?}");
}

#[test]
fn ordinary_sql_errors_are_not_storage_errors() {
    let s = Store::open_in_memory().unwrap();
    let err: StoreError = s
        .conn()
        .execute("INSERT INTO no_such_table VALUES(1)", [])
        .unwrap_err()
        .into();
    assert!(matches!(err, StoreError::Sql(_)), "{err:?}");
    assert_eq!(err.storage_kind(), None);
}
