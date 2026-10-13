//! 故障注入的小工具（测试用；集成测试也要用，所以不放在 `cfg(test)` 里）。只碰调用方给的文件。
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// 把库文件末尾往前的三整页涂成 0xFF：库还打得开（第一页的头没动），但 `quick_check` 会报错
/// （模拟悄悄坏掉的页）。调用前库要已经 checkpoint 进主文件、连接全部关掉；库要有几十页以上的数据。
pub fn scribble_near_end(db: &Path) {
    let mut f = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    let len = f.seek(SeekFrom::End(0)).unwrap();
    assert!(len > 4096 * 8, "库太小，涂不到数据页：{len}");
    f.seek(SeekFrom::Start(len - 4096 * 4)).unwrap();
    f.write_all(&[0xff; 4096 * 3]).unwrap();
    f.sync_all().unwrap();
}

/// 把文件截成一半（模拟拷了一半 / 写了一半的库）。
pub fn truncate_half(db: &Path) {
    let f = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    let len = f.metadata().unwrap().len();
    f.set_len(len / 2).unwrap();
}

/// 只坏索引、表数据完好：`VACUUM INTO` 照样拷得出来（并把坏处一起拷走），`integrity_check` 才看得见。
/// 用来证明「检查」这一步自己在起作用，而不是被后面更早失败的步骤顶替。
pub fn corrupt_an_index(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "CREATE TABLE probe(a TEXT, b TEXT);
         CREATE INDEX probe_i ON probe(a);
         INSERT INTO probe VALUES('x','y'),('p','q');
         PRAGMA writable_schema=ON;
         UPDATE sqlite_master SET sql='CREATE INDEX probe_i ON probe(b)' WHERE name='probe_i';
         PRAGMA writable_schema=OFF;",
    )
    .unwrap();
}

/// 让一行违反 CHECK 约束（改表定义，不改数据）：`quick_check` 报错，`VACUUM INTO` 却照样拷得出来
/// （VACUUM 不核 CHECK）——用来证明「拍完的那份自己也要过检查」这一步在起作用。
pub fn violate_a_check(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "CREATE TABLE probe_check(a INTEGER);
         INSERT INTO probe_check VALUES(5);
         PRAGMA writable_schema=ON;
         UPDATE sqlite_master SET sql='CREATE TABLE probe_check(a INTEGER CHECK(a<3))'
          WHERE name='probe_check';
         PRAGMA writable_schema=OFF;",
    )
    .unwrap();
}

/// 一个停在 v2、有数据、没有 dbId 的库（= N4 之前的库，或它的迁移前快照）。只用合成数据。
pub fn write_v2_db(path: &Path) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(crate::schema::V1).unwrap();
    c.execute_batch(crate::schema::V2).unwrap();
    c.execute_batch(
        "INSERT INTO crews(id,title,created_at,updated_at) VALUES('old','示例','t','t');
         INSERT INTO messages(id,crew_id,sender_kind,text,created_at) VALUES('m1','old','user','示例','t');",
    )
    .unwrap();
}

/// 造一份「崩溃留下的」非空 WAL：在 `db` 上提交 `sql`（不做检查点），连接还开着时把 -wal / -shm 拷出来，
/// 返回拷出来的两份（关连接时 SQLite 会把原来的折回去删掉，拷出来的这两份就是崩溃时留在盘上的样子）。
pub fn crash_left_wal(
    db: &Path,
    sql: &str,
    out_dir: &Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let c = rusqlite::Connection::open(db).unwrap();
    c.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
    c.execute_batch(sql).unwrap();
    let wal = out_dir.join("stale-wal");
    let shm = out_dir.join("stale-shm");
    let side = |s: &str| {
        let mut p = db.as_os_str().to_owned();
        p.push(s);
        std::path::PathBuf::from(p)
    };
    std::fs::copy(side("-wal"), &wal).unwrap();
    std::fs::copy(side("-shm"), &shm).unwrap();
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    (wal, shm)
}

/// Fresh synthetic historical v8 store for migration fixtures. It runs the true
/// historical chain; it neither opens nor downgrades an existing database.
/// Live organization tests must use the current Store constructors instead.
pub fn open_v8_store() -> crate::Result<crate::Store> {
    open_legacy_store(8)
}

/// Empty caller-owned historical v4–v8 store; never a downgrade of an existing
/// database. Use only for old-schema migration samples, not live product tests.
pub fn open_legacy_store(version: i64) -> crate::Result<crate::Store> {
    let conn = rusqlite::Connection::open_in_memory()?;
    crate::pin_pragmas(&conn)?;
    crate::schema::initialize_legacy_fixture(&conn, version)?;
    crate::mark_identity(&conn)?;
    Ok(crate::Store {
        conn,
        backups: None,
    })
}

#[cfg(test)]
mod historical_fixture_tests {
    #[test]
    fn v8_fixture_has_real_old_authority_and_refuses_reinitialization() {
        let store = super::open_v8_store().unwrap();
        let version: i64 = store
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 8);
        let table: String = store
            .conn()
            .query_row(
                "SELECT type FROM sqlite_master WHERE name='work_books'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, "table");
        let organizations: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(organizations, 0);
        assert!(crate::schema::initialize_v8_fixture(store.conn()).is_err());
        assert_eq!(
            store
                .conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            8
        );
    }
    #[test]
    fn historical_versions_stop_at_their_actual_tables_without_fake_downgrades() {
        for version in 4..=8 {
            let store = super::open_legacy_store(version).unwrap();
            let actual: i64 = store
                .conn()
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(actual, version);
            for (table, introduced) in [
                ("object_refs", 5),
                ("work_events", 6),
                ("work_stop_checks", 7),
                ("runner_usage", 8),
            ] {
                let exists: i64 = store
                    .conn()
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(exists, i64::from(version >= introduced));
            }
        }
        assert!(super::open_legacy_store(9).is_err());
    }
}
