//! 在线备份、轮转、定时拍、恢复（N4 / S21）。临时目录 + 合成数据。
use crewd_store::backup::{self, Policy, Ticker};
use crewd_store::{DB_FILE, ID_FILE, Store, StoreError, crews, meta_get};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DAY: i64 = 86_400_000;

fn add_crew(s: &Store, id: &str) {
    crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: id.into(),
            title: format!("示例 {id}"),
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
}

fn crew_ids(db: &Path) -> Vec<String> {
    let c = rusqlite::Connection::open(db).unwrap();
    let mut st = c.prepare("SELECT id FROM crews ORDER BY id").unwrap();
    st.query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn files(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<String> = rd
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn an_online_backup_is_a_checked_0600_copy_taken_beside_a_live_writer() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open_data_dir(d.path()).unwrap();
    add_crew(&s, "a");
    // 写者开着、库线程那条连接不动：备份走自己的只读连接。
    let dest = d.path().join("backups/manual.db");
    backup::backup_db(&d.path().join(DB_FILE), &dest).unwrap();
    add_crew(&s, "b");
    assert_eq!(crew_ids(&dest), ["a"]);
    let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{mode:o}");
    // 绝不覆盖。
    assert!(backup::backup_db(&d.path().join(DB_FILE), &dest).is_err());
    assert_eq!(crew_ids(&dest), ["a"]);
}

#[test]
fn backing_up_a_missing_db_does_not_create_one() {
    let d = tempfile::tempdir().unwrap();
    assert!(backup::backup_db(&d.path().join(DB_FILE), &d.path().join("b.db")).is_err());
    assert!(!d.path().join(DB_FILE).exists());
}

fn auto(dir: &Path, ms: i64) -> String {
    let name = backup::auto_name(ms);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(&name), b"x").unwrap();
    name
}

#[test]
fn rotation_keeps_seven_days_and_four_weeks_and_never_touches_other_snapshots() {
    let d = tempfile::tempdir().unwrap();
    let b = d.path().join("backups");
    // 2026-10-07 12:00 UTC（周三）往前 60 天，每天两份（早、晚）。
    let now = 1_791_374_400_000;
    let mut all = vec![];
    for day in 0..60 {
        for h in [1, 13] {
            all.push((
                now - day * DAY - h * 3_600_000,
                auto(&b, now - day * DAY - h * 3_600_000),
            ));
        }
    }
    std::fs::write(b.join("crewd-manual-20260101T000000000Z.db"), b"m").unwrap();
    std::fs::write(b.join("crewd-v2-pre-v3-20250101T000000000Z.db"), b"p").unwrap();
    std::fs::write(
        b.join("crewd-pre-delete-crew-a-20250101T000000000Z.db"),
        b"p",
    )
    .unwrap();
    let removed = backup::rotate(&b, Policy::default()).unwrap();
    let kept: Vec<String> = files(&b)
        .into_iter()
        .filter(|n| n.starts_with("crewd-auto-"))
        .collect();
    // 每天留最新的那份：最近 7 天 = 7 份；比它们更早的再按 ISO 周各留最新一份、共 4 周。
    assert_eq!(kept.len(), 11, "{kept:?}");
    // 最近 7 个（UTC）日子，各自最新的那份都在。
    let mut by_day: std::collections::BTreeMap<i64, i64> = Default::default();
    for (ms, _) in &all {
        let day = ms.div_euclid(DAY);
        let e = by_day.entry(day).or_insert(*ms);
        *e = (*e).max(*ms);
    }
    for (day, newest) in by_day.iter().rev().take(7) {
        let name = backup::auto_name(*newest);
        assert!(kept.contains(&name), "第 {day} 天：{kept:?}");
    }
    assert_eq!(removed.len() + kept.len(), all.len());
    for keep in [
        "crewd-manual-20260101T000000000Z.db",
        "crewd-v2-pre-v3-20250101T000000000Z.db",
        "crewd-pre-delete-crew-a-20250101T000000000Z.db",
    ] {
        assert!(b.join(keep).exists(), "{keep}");
    }
    // 再转一次不动（幂等）。
    assert!(backup::rotate(&b, Policy::default()).unwrap().is_empty());
}

#[test]
fn a_periodic_backup_is_due_once_a_day() {
    let d = tempfile::tempdir().unwrap();
    let b = d.path().join("backups");
    let now = 1_791_374_400_000;
    let p = Policy::default();
    assert!(backup::due(&b, now, p).unwrap(), "一份都没有");
    auto(&b, now - 3_600_000);
    assert!(!backup::due(&b, now, p).unwrap());
    assert!(backup::due(&b, now + DAY, p).unwrap());
}

#[test]
fn a_tick_backs_up_checks_and_rotates() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join(DB_FILE);
    let s = Store::open_data_dir(d.path()).unwrap();
    let b = d.path().join("backups");
    let now = 1_791_374_400_000;
    // 空库：没有要保的，不拍。
    let r = backup::tick(&db, now, Policy::default()).unwrap();
    assert!(r.backup.is_none() && r.live_problems.is_empty());
    add_crew(&s, "a");
    // 30 份更早的定时备份（第 2..=31 天前各一份）：拍完新的之后最老的那些被轮转掉。
    for day in 2..=31 {
        auto(&b, now - day * DAY);
    }
    let oldest = b.join(backup::auto_name(now - 31 * DAY));
    let r = backup::tick(&db, now, Policy::default()).unwrap();
    let made = r.backup.clone().expect("该拍了");
    assert_eq!(crew_ids(&made), ["a"]);
    assert!(r.removed.contains(&oldest), "{:?}", r.removed);
    assert!(!oldest.exists());
    assert!(made.exists());
    // 同一天再 tick：不重复拍。
    assert!(
        backup::tick(&db, now + 60_000, Policy::default())
            .unwrap()
            .backup
            .is_none()
    );
}

#[test]
fn a_tick_on_a_db_failing_quick_check_reports_and_does_not_rotate() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join(DB_FILE);
    {
        let s = Store::open_data_dir(d.path()).unwrap();
        add_crew(&s, "a");
        s.conn()
            .execute_batch(
                "CREATE TABLE filler(id INTEGER PRIMARY KEY, t TEXT);
                 WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<5000)
                 INSERT INTO filler(t) SELECT hex(randomblob(50)) FROM n;",
            )
            .unwrap();
    }
    crewd_store::testing::scribble_near_end(&db);
    let b = d.path().join("backups");
    let now = 1_791_374_400_000;
    for day in 2..=31 {
        auto(&b, now - day * DAY);
    }
    let before = backup::list_auto(&b).unwrap().len();
    let r = backup::tick(&db, now, Policy::default()).unwrap();
    assert!(!r.live_problems.is_empty(), "坏处要报出来");
    assert!(r.removed.is_empty(), "库没过检查时不淘汰旧备份");
    assert!(backup::list_auto(&b).unwrap().len() >= before);
    assert!(db.exists(), "绝不删库");
}

#[test]
fn the_ticker_backs_up_on_start_and_stops_cleanly() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join(DB_FILE);
    let s = Store::open_data_dir(d.path()).unwrap();
    add_crew(&s, "a");
    let t = Ticker::start(db.clone(), Duration::from_millis(20), Policy::default()).unwrap();
    let b = d.path().join("backups");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while backup::list_auto(&b).unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "定时备份没出现");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    t.stop();
    assert_eq!(backup::list_auto(&b).unwrap().len(), 1, "一天只拍一份");
}

fn backup_of(dir: &Path) -> PathBuf {
    let dest = dir.join("backups/crewd-manual-x.db");
    backup::backup_db(&dir.join(DB_FILE), &dest).unwrap();
    dest
}

#[test]
fn restore_moves_the_current_db_aside_and_brings_the_backup_back() {
    let d = tempfile::tempdir().unwrap();
    let id;
    let src = {
        let s = Store::open_data_dir(d.path()).unwrap();
        id = meta_get(s.conn(), "dbId").unwrap().unwrap();
        add_crew(&s, "a");
        let src = backup_of(d.path());
        add_crew(&s, "b");
        src
    };
    let r = backup::restore(d.path(), &src).unwrap();
    let replaced = r.replaced.expect("原来的库挪到一边");
    assert!(
        replaced
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("crewd.db.replaced-")
    );
    assert_eq!(
        crew_ids(&replaced),
        ["a", "b"],
        "被替换的库原样保留（含 WAL 里的提交）"
    );
    let s = Store::open_data_dir(d.path()).unwrap();
    let ids: Vec<String> = crews::list(s.conn())
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, ["a"]);
    assert_eq!(
        std::fs::read_to_string(d.path().join(ID_FILE))
            .unwrap()
            .trim(),
        id
    );
    assert!(src.exists(), "备份文件本身不动");
}

#[test]
fn restore_brings_back_a_lost_db() {
    let d = tempfile::tempdir().unwrap();
    let src = {
        let s = Store::open_data_dir(d.path()).unwrap();
        add_crew(&s, "a");
        backup_of(d.path())
    };
    for f in ["crewd.db", "crewd.db-wal", "crewd.db-shm"] {
        let _ = std::fs::remove_file(d.path().join(f));
    }
    assert!(matches!(
        Store::open_data_dir(d.path()),
        Err(StoreError::DbMissing { .. })
    ));
    let r = backup::restore(d.path(), &src).unwrap();
    assert!(r.replaced.is_none());
    let s = Store::open_data_dir(d.path()).unwrap();
    assert_eq!(crews::list(s.conn()).unwrap().len(), 1);
}

#[test]
fn restore_refuses_bad_sources_and_leaves_the_current_db_alone() {
    let d = tempfile::tempdir().unwrap();
    {
        let s = Store::open_data_dir(d.path()).unwrap();
        add_crew(&s, "a");
    }
    let bad = d.path().join("not-a-db.db");
    std::fs::write(&bad, vec![0x5au8; 8192]).unwrap();
    let corrupt = d.path().join("corrupt.db");
    {
        let s = Store::open(&corrupt).unwrap();
        add_crew(&s, "z");
        crewd_store::testing::corrupt_an_index(s.conn());
    }
    let newer = d.path().join("newer.db");
    {
        let s = Store::open(&newer).unwrap();
        s.conn().execute_batch("PRAGMA user_version=99").unwrap();
    }
    for src in [&bad, &corrupt, &newer, &d.path().join("missing.db")] {
        assert!(backup::restore(d.path(), src).is_err(), "{}", src.display());
    }
    assert!(
        backup::restore(d.path(), &d.path().join(DB_FILE)).is_err(),
        "不能拿库自己恢复自己"
    );
    assert_eq!(crew_ids(&d.path().join(DB_FILE)), ["a"]);
    assert!(
        !files(d.path()).iter().any(|n| n.contains("replaced")),
        "{:?}",
        files(d.path())
    );
}

/// 拍出来的那份没过 quick_check 就不算备份：报错、不留文件（VACUUM INTO 本身是成功的）。
#[test]
fn a_snapshot_that_fails_its_own_quick_check_is_not_kept() {
    let d = tempfile::tempdir().unwrap();
    {
        let s = Store::open_data_dir(d.path()).unwrap();
        add_crew(&s, "a");
        crewd_store::testing::violate_a_check(s.conn());
    }
    let dest = d.path().join("backups/x.db");
    let err = backup::backup_db(&d.path().join(DB_FILE), &dest).unwrap_err();
    assert_eq!(
        err.storage_kind(),
        Some(crewd_store::StorageKind::Corrupt),
        "{err}"
    );
    assert!(
        files(&d.path().join("backups")).is_empty(),
        "不留半截 / 坏的备份"
    );
}

/// S22（store 侧）：永久删除之前的快照落在 backups/crewd-pre-delete-{对象}-{时间}.db，名字里的对象 id
/// 只留安全字符；内存库没有备份目录就不拍（返回 None）；这类快照不参加轮转。
#[test]
fn a_pre_purge_snapshot_lands_in_backups_and_is_never_rotated() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open_data_dir(d.path()).unwrap();
    add_crew(&s, "a");
    let p = s
        .snapshot_before_purge("crew/../a b")
        .unwrap()
        .expect("文件库要拍");
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("crewd-pre-delete-crew_.._a_b-"), "{name}");
    assert_eq!(p.parent().unwrap(), d.path().join("backups"));
    assert_eq!(crew_ids(&p), ["a"]);
    assert!(
        backup::rotate(&d.path().join("backups"), Policy::default())
            .unwrap()
            .is_empty()
    );
    assert!(p.exists());
    let mem = Store::open_in_memory().unwrap();
    assert!(mem.snapshot_before_purge("a").unwrap().is_none());
}
