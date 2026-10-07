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
            .lines()
            .next()
            .unwrap(),
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
            .lines()
            .next()
            .unwrap(),
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
    let names_before = sidecars(d.path());
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("没有 dbId 的库不能顶替");
    assert!(matches!(err, StoreError::DbUnmarked { .. }), "{err:?}");
    // 复审 2 轮 4：偷看不留 -wal / -shm（这是个 WAL 头的文件）。
    assert_eq!(sidecars(d.path()), names_before, "偷看不建旁路文件");
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

fn side(dir: &Path, suffix: &str) -> std::path::PathBuf {
    dir.join(format!("{DB_FILE}{suffix}"))
}

/// 复审 2 轮 3：A 崩溃留下的 WAL（里面有 A 的 meta 页）还在，crewd.db 换成了另一个库 B。偷看要只看主文件，
/// 不能透过 A 的 WAL 读到 A 的 dbId 而放行（放行 = 把 A 的页回放到 B 上）。拒绝，B 和旧 WAL 都不动。
#[test]
fn a_stale_wal_cannot_lend_its_identity_to_a_swapped_db() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    db_id(a.path());
    db_id(b.path());
    let (wal, shm) = crewd_store::testing::crash_left_wal(
        &a.path().join(DB_FILE),
        "UPDATE meta SET value=value WHERE key='dbId';
         INSERT INTO meta(key,value) VALUES('probe','x');",
        scratch.path(),
    );
    // `cp b a` 式的覆盖：同一个文件（inode 不变）、内容换成 B——inode 判据看不出来，只能靠只读主文件的偷看。
    std::fs::write(
        a.path().join(DB_FILE),
        std::fs::read(b.path().join(DB_FILE)).unwrap(),
    )
    .unwrap();
    std::fs::copy(&wal, side(a.path(), "-wal")).unwrap();
    std::fs::copy(&shm, side(a.path(), "-shm")).unwrap();
    let main_before = std::fs::read(a.path().join(DB_FILE)).unwrap();
    let wal_before = std::fs::read(side(a.path(), "-wal")).unwrap();
    let err = Store::open_data_dir(a.path())
        .err()
        .expect("换进来的库不能借旧 WAL 的身份");
    assert!(matches!(err, StoreError::DbIdMismatch { .. }), "{err:?}");
    assert_eq!(std::fs::read(a.path().join(DB_FILE)).unwrap(), main_before);
    assert_eq!(std::fs::read(side(a.path(), "-wal")).unwrap(), wal_before);
}

/// 同一个库的旧副本被 `mv` 进来（dbId 一样），而上次崩溃的 WAL 还在：库文件换过了（inode 变了），
/// 旧 WAL 不属于它。拒绝，文件不动。
#[test]
fn an_older_copy_moved_in_over_a_crash_left_wal_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    db_id(d.path());
    let older = scratch.path().join("older.db");
    crewd_store::backup::backup_db(&d.path().join(DB_FILE), &older).unwrap();
    let (wal, shm) = crewd_store::testing::crash_left_wal(
        &d.path().join(DB_FILE),
        "INSERT INTO meta(key,value) VALUES('probe','x');",
        scratch.path(),
    );
    remove_db(d.path());
    std::fs::rename(&older, d.path().join(DB_FILE)).unwrap();
    std::fs::copy(&wal, side(d.path(), "-wal")).unwrap();
    std::fs::copy(&shm, side(d.path(), "-shm")).unwrap();
    let wal_before = std::fs::read(side(d.path(), "-wal")).unwrap();
    let err = Store::open_data_dir(d.path())
        .err()
        .expect("旧 WAL 不属于换进来的文件");
    assert!(matches!(err, StoreError::DbReplaced { .. }), "{err:?}");
    assert!(err.to_string().contains("-wal"), "{err}");
    // 整个数据目录搬过家时 crewd.db 和 -wal 是一对：提示要说认领（删 crewd.db.id），绝不删 -wal。
    for want in ["Time Machine", "删掉 crewd.db.id", "绝不要删 crewd.db-wal"] {
        assert!(err.to_string().contains(want), "缺 {want}：{err}");
    }
    assert_eq!(std::fs::read(side(d.path(), "-wal")).unwrap(), wal_before);
}

/// 反面：真崩溃（同一个文件、它自己的 WAL）照常打开，WAL 里的提交都在。
#[test]
fn a_genuine_crash_left_wal_is_recovered() {
    let d = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    db_id(d.path());
    let main_before_crash = std::fs::read(d.path().join(DB_FILE)).unwrap();
    let (wal, shm) = crewd_store::testing::crash_left_wal(
        &d.path().join(DB_FILE),
        "INSERT INTO meta(key,value) VALUES('probe','x');",
        scratch.path(),
    );
    // 关连接时 SQLite 已经把 WAL 折回去了；把崩溃时的样子摆回来：同一个文件（同 inode）的内容回到
    // 崩溃前，WAL 放回去。probe 只在 WAL 里。
    std::fs::write(d.path().join(DB_FILE), &main_before_crash).unwrap();
    std::fs::copy(&wal, side(d.path(), "-wal")).unwrap();
    std::fs::copy(&shm, side(d.path(), "-shm")).unwrap();
    let s = Store::open_data_dir(d.path()).unwrap();
    assert_eq!(meta_get(s.conn(), "probe").unwrap().as_deref(), Some("x"));
}

/// 第一次建库：标识文件写下之前，dbId 已经落进主文件（偷看只读主文件，所以这是前提）。
#[test]
fn the_db_id_reaches_the_main_file_before_the_id_file_is_written() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open_data_dir(d.path()).unwrap();
    let id = meta_get(s.conn(), "dbId").unwrap().unwrap();
    let uri = format!("file:{}?immutable=1", d.path().join(DB_FILE).display());
    let c = rusqlite::Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let main: String = c
        .query_row("SELECT value FROM meta WHERE key='dbId'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(main, id);
    drop(s);
}
