//! 一次小提交的落盘代价：`fullfsync` 关 / 开（N4，S23「先测再定」）。不进 check.sh：
//! `cargo test -p crewd-store --release --test fsync_cost -- --ignored --nocapture`
//! 库写在 `$TMPDIR`（和数据目录同一个 APFS 卷）；只用合成数据。
use rusqlite::Connection;
use std::time::{Duration, Instant};

fn run(fullfsync: bool, n: usize) -> Vec<Duration> {
    let dir = tempfile::tempdir().unwrap();
    let c = Connection::open(dir.path().join("bench.db")).unwrap();
    let _: String = c
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .unwrap();
    c.execute_batch(&format!(
        "PRAGMA synchronous=FULL; PRAGMA checkpoint_fullfsync=ON; PRAGMA fullfsync={};
         CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT NOT NULL);",
        if fullfsync { "ON" } else { "OFF" }
    ))
    .unwrap();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // 和游标推进、wakes 结算同一个量级：一个事务、一行。
        let t = Instant::now();
        c.execute("INSERT INTO t(v) VALUES(?1)", [format!("示例 {i}")])
            .unwrap();
        out.push(t.elapsed());
    }
    out.sort();
    out
}

fn pct(v: &[Duration], p: f64) -> Duration {
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
#[ignore = "基准：手动跑，读数记进 N4 报告"]
fn commit_cost_with_and_without_fullfsync() {
    let n = 400;
    for round in 0..2 {
        for on in [false, true] {
            let v = run(on, n);
            println!(
                "round={round} fullfsync={} n={n} p50={:?} p90={:?} p99={:?} max={:?}",
                if on { "ON" } else { "OFF" },
                pct(&v, 0.5),
                pct(&v, 0.9),
                pct(&v, 0.99),
                v[v.len() - 1]
            );
        }
    }
}
