//! 真实量级（M3a 演练：367066 条消息）的合成库上量：quick_check / integrity_check / 快照 / 恢复源检查
//! 各要多久（N4，S25「先量再定启动时跑不跑」）。不进 check.sh：
//! `cargo test -p crewd-store --release --test check_cost -- --ignored --nocapture`
use crewd_store::{Store, backup, crews};
use std::time::Instant;

#[test]
#[ignore = "基准：手动跑，读数记进 N4 报告"]
fn check_and_snapshot_cost_at_real_scale() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join("crewd.db");
    let s = Store::open_data_dir(d.path()).unwrap();
    crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: "c".into(),
            title: "示例".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    let t = Instant::now();
    // 平均 ~300 字节的正文（合成）。
    s.conn()
        .execute_batch(
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<367066)
             INSERT INTO messages(id,channel_id,sender_kind,text,created_at)
             SELECT 'm'||x, 'c/lobby', 'human', hex(randomblob(150)),
                    strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM n;",
        )
        .unwrap();
    println!("seed: {:?}", t.elapsed());
    let size = backup::db_bytes(s.conn()).unwrap();
    println!("db size: {:.1} MiB", size as f64 / 1048576.0);
    let c = backup::open_for_check(&db).unwrap();
    for kind in [
        backup::Check::Quick,
        backup::Check::Integrity,
        backup::Check::Full,
    ] {
        let t = Instant::now();
        let p = backup::check(&c, kind).unwrap();
        println!("{}: {:?} (problems={})", kind.name(), t.elapsed(), p.len());
    }
    let t = Instant::now();
    backup::backup_db(&db, &d.path().join("backups/x.db")).unwrap();
    println!(
        "backup_db (VACUUM INTO + quick_check of the copy + fsync): {:?}",
        t.elapsed()
    );
    let t = Instant::now();
    backup::vacuum_into(s.conn(), &d.path().join("backups/y.db")).unwrap();
    println!("vacuum_into on the writer connection: {:?}", t.elapsed());
}
