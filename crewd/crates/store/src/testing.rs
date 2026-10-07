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
