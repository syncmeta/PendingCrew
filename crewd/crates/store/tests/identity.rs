//! 库的身份（S24）：该在的库不在、或者换成了另一个库时拒绝启动，绝不悄悄新建一个空库。
use crewd_store::{APPLICATION_ID, DB_FILE, ID_FILE, Store, StoreError, meta_get};
use std::path::Path;

fn db_id(dir: &Path) -> String {
    let s = Store::open_data_dir(dir).unwrap();
    meta_get(s.conn(), "dbId").unwrap().unwrap()
}

fn remove_db(dir: &Path) {
    for f in ["crewd.db", "crewd.db-wal", "crewd.db-shm"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

#[test]
fn first_open_marks_the_db_and_writes_the_id_file() {
    let d = tempfile::tempdir().unwrap();
    let id = db_id(d.path());
    assert_eq!(id.len(), 32, "{id}");
    assert_eq!(
        std::fs::read_to_string(d.path().join(ID_FILE))
            .unwrap()
            .trim(),
        id
    );
    let s = Store::open_data_dir(d.path()).unwrap();
    let app: i32 = s
        .conn()
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(app, APPLICATION_ID);
    drop(s);
    assert_eq!(db_id(d.path()), id, "再开还是同一个库");
}

#[test]
fn a_missing_db_with_an_id_file_refuses_to_start_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    db_id(d.path());
    remove_db(d.path());
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("库丢了必须拒绝");
    assert!(matches!(err, StoreError::DbMissing { .. }), "{err:?}");
    assert!(err.to_string().contains("crewd restore"), "{err}");
    assert!(
        err.to_string().contains("-wal"),
        "要说连 -wal / -shm 一起挪：{err}"
    );
    assert!(!d.path().join(DB_FILE).exists(), "不能顺手建出空库");
}

#[test]
fn a_missing_db_next_to_backups_refuses_even_without_the_id_file() {
    let d = tempfile::tempdir().unwrap();
    db_id(d.path());
    remove_db(d.path());
    std::fs::remove_file(d.path().join(ID_FILE)).unwrap();
    std::fs::create_dir(d.path().join("backups")).unwrap();
    std::fs::write(d.path().join("backups/crewd-20261001T000000000Z.db"), b"x").unwrap();
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("有备份没库 = 丢了");
    assert!(matches!(err, StoreError::DbMissing { .. }), "{err:?}");
    assert!(!d.path().join(DB_FILE).exists());
}

#[test]
fn a_swapped_db_refuses_to_start() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    db_id(a.path());
    db_id(b.path());
    remove_db(a.path());
    std::fs::copy(b.path().join(DB_FILE), a.path().join(DB_FILE)).unwrap();
    let err = Store::open_data_dir(a.path())
        .err()
        .expect("换了库必须拒绝");
    assert!(matches!(err, StoreError::DbIdMismatch { .. }), "{err:?}");
}

#[test]
fn a_pre_marker_db_is_adopted() {
    let d = tempfile::tempdir().unwrap();
    // N4 之前建的库：没有 id 文件，meta 里也没有 dbId。
    {
        let s = Store::open(&d.path().join(DB_FILE)).unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key='dbId'", [])
            .unwrap();
        s.conn().execute_batch("PRAGMA application_id=0").unwrap();
    }
    let id = db_id(d.path());
    assert_eq!(
        std::fs::read_to_string(d.path().join(ID_FILE))
            .unwrap()
            .trim(),
        id
    );
}

#[test]
fn a_foreign_sqlite_file_is_refused() {
    let d = tempfile::tempdir().unwrap();
    {
        let c = rusqlite::Connection::open(d.path().join(DB_FILE)).unwrap();
        c.execute_batch("CREATE TABLE notes(t TEXT); INSERT INTO notes VALUES('x');")
            .unwrap();
    }
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("别人的库不能碰");
    assert!(err.to_string().contains("不是 crewd"), "{err}");
    // M1：拒绝之前什么 pragma 都没设：还是回滚日志模式（文件头第 18、19 字节 = 1），没有 -wal。
    let header = std::fs::read(d.path().join(DB_FILE)).unwrap();
    assert_eq!((header[18], header[19]), (1, 1), "没被改成 WAL");
    assert!(!d.path().join("crewd.db-wal").exists());
    let c = rusqlite::Connection::open(d.path().join(DB_FILE)).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "一张表都没多建");

    let e = tempfile::tempdir().unwrap();
    {
        let c = rusqlite::Connection::open(e.path().join(DB_FILE)).unwrap();
        c.execute_batch("PRAGMA application_id=1234").unwrap();
    }
    let err = Store::open_data_dir(e.path())
        .err()
        .expect("别的程序标过的库不能碰");
    assert!(err.to_string().contains("不是 crewd"), "{err}");
}

fn sidecars(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// 复审 1：有人把迁移前快照（旧版本、没有 dbId）`cp` 回 crewd.db。必须在打开之前就拒绝：
/// 不设 pragma、不迁移、不补 dbId，文件一个字节都不动；提示怎么恢复 / 认领。
#[test]
fn an_id_less_old_db_copied_in_is_refused_before_anything_touches_it() {
    let d = tempfile::tempdir().unwrap();
    db_id(d.path());
    remove_db(d.path());
    let db = d.path().join(DB_FILE);
    crewd_store::testing::write_v2_db(&db);
    let before = std::fs::read(&db).unwrap();
    let names_before = sidecars(d.path());
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("没有 dbId 的库不能顶替");
    assert!(matches!(err, StoreError::DbUnmarked { .. }), "{err:?}");
    let msg = err.to_string();
    for want in ["crewd restore --from", ID_FILE, "crewd.db.replaced-"] {
        assert!(msg.contains(want), "缺 {want}：{msg}");
    }
    assert_eq!(std::fs::read(&db).unwrap(), before, "字节不动");
    assert_eq!(
        sidecars(d.path()),
        names_before,
        "不建 -wal / -shm / backups"
    );
}

#[test]
fn an_id_less_current_db_copied_in_is_refused_too() {
    let d = tempfile::tempdir().unwrap();
    db_id(d.path());
    let other = tempfile::tempdir().unwrap();
    {
        let s = Store::open(&other.path().join(DB_FILE)).unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key='dbId'", [])
            .unwrap();
    }
    remove_db(d.path());
    std::fs::copy(other.path().join(DB_FILE), d.path().join(DB_FILE)).unwrap();
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("没有 dbId 的库不能顶替");
    assert!(matches!(err, StoreError::DbUnmarked { .. }), "{err:?}");
}

/// id 文件在、库文件却是空的（0 字节）：等于库丢了，不在里面建表。
#[test]
fn an_empty_db_file_next_to_the_id_file_counts_as_missing() {
    let d = tempfile::tempdir().unwrap();
    db_id(d.path());
    remove_db(d.path());
    std::fs::write(d.path().join(DB_FILE), b"").unwrap();
    let err = Store::open_data_dir(d.path()).err().expect("空文件 = 丢了");
    assert!(matches!(err, StoreError::DbMissing { .. }), "{err:?}");
    assert_eq!(std::fs::metadata(d.path().join(DB_FILE)).unwrap().len(), 0);
}

/// 换进来的库标识对不上时，提示同样要说清怎么办。
#[test]
fn the_mismatch_message_says_what_to_do() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    db_id(a.path());
    db_id(b.path());
    remove_db(a.path());
    std::fs::copy(b.path().join(DB_FILE), a.path().join(DB_FILE)).unwrap();
    let msg = Store::open_data_dir(a.path()).err().unwrap().to_string();
    for want in [
        "crewd restore --from",
        ID_FILE,
        "crewd.db.replaced-",
        "挪出",
        "-wal",
    ] {
        assert!(msg.contains(want), "缺 {want}：{msg}");
    }
}

/// 换进来的是另一个库的旧版本（带着别的 dbId）：在迁移之前就拒绝，文件一个字节都不动。
#[test]
fn a_swapped_old_version_db_is_refused_before_it_is_migrated() {
    let d = tempfile::tempdir().unwrap();
    db_id(d.path());
    remove_db(d.path());
    let db = d.path().join(DB_FILE);
    crewd_store::testing::write_v2_db(&db);
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO meta(key,value) VALUES('dbId','0123456789abcdef0123456789abcdef')",
            [],
        )
        .unwrap();
    let before = std::fs::read(&db).unwrap();
    let names_before = sidecars(d.path());
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("换了库必须拒绝");
    assert!(matches!(err, StoreError::DbIdMismatch { .. }), "{err:?}");
    assert_eq!(std::fs::read(&db).unwrap(), before, "字节不动（没迁移）");
    assert_eq!(sidecars(d.path()), names_before);
}
