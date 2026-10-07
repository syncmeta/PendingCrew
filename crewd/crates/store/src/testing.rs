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
