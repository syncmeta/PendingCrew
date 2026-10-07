//! 权限（PM 2026-10-07）：库、-wal、-shm、备份文件都是 0600（聊天记录，别让同机其他用户读）。
use crewd_store::{Store, backup, crews};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn touch_a_row(s: &Store) {
    crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: "a".into(),
            title: "示例".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
}

#[test]
fn a_new_db_and_its_wal_and_shm_are_0600() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("crewd.db");
    let s = Store::open(&p).unwrap();
    touch_a_row(&s);
    for f in ["crewd.db", "crewd.db-wal", "crewd.db-shm"] {
        let path = d.path().join(f);
        assert!(path.exists(), "{f}");
        assert_eq!(mode(&path), 0o600, "{f}: {:o}", mode(&path));
    }
}

#[test]
fn an_existing_loose_db_is_tightened_on_open() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("crewd.db");
    // N4 之前建的库是 umask 给的 0644。
    drop(Store::open(&p).unwrap());
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    let s = Store::open(&p).unwrap();
    touch_a_row(&s);
    for f in ["crewd.db", "crewd.db-wal", "crewd.db-shm"] {
        let path = d.path().join(f);
        assert_eq!(mode(&path), 0o600, "{f}: {:o}", mode(&path));
    }
}

#[test]
fn snapshots_are_0600() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("crewd.db")).unwrap();
    touch_a_row(&s);
    let dest = d.path().join("backups/snap.db");
    backup::vacuum_into(s.conn(), &dest).unwrap();
    assert_eq!(mode(&dest), 0o600, "{:o}", mode(&dest));
    assert_eq!(mode(&d.path().join("backups")), 0o700);
}
