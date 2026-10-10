//! N4 存储安全的命令行端到端：库丢了拒绝启动、备份、恢复、检查。临时数据目录 + 合成数据。
use std::path::Path;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

// Each binary test process owns a synthetic home outside every data directory.
static TEST_HOME: std::sync::LazyLock<tempfile::TempDir> =
    std::sync::LazyLock::new(|| tempfile::tempdir().unwrap());

fn crewd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.env("CREWD_DATA_DIR", data)
        .env_remove("PENDINGCREW_DATA_DIR")
        .env("CREWD_CLAUDE", "/usr/bin/false")
        .env("SHELL", "/usr/bin/false")
        .env("HOME", TEST_HOME.path())
        .env("XDG_CACHE_HOME", TEST_HOME.path().join("cache"))
        .env("CLAUDE_CONFIG_DIR", TEST_HOME.path().join(".claude"));
    c
}

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(data: &Path) -> Daemon {
    let d = Daemon(crewd(data).arg("serve").spawn().unwrap());
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        if crewd(data)
            .args(["rpc", "system.hello"])
            .output()
            .unwrap()
            .status
            .success()
        {
            return d;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("crewd serve 没起来");
}

fn ok(out: Output) -> Output {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// 第一次起服务：建库 + 标识文件；停掉。
fn init_dir(data: &Path) {
    drop(start(data));
    assert!(data.join("crewd.db.id").exists());
}

#[test]
fn serve_refuses_to_start_when_the_db_is_gone() {
    let data = tempfile::tempdir().unwrap();
    init_dir(data.path());
    for f in ["crewd.db", "crewd.db-wal", "crewd.db-shm"] {
        let _ = std::fs::remove_file(data.path().join(f));
    }
    let out = crewd(data.path()).arg("serve").output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("crewd restore"), "{}", stderr(&out));
    assert!(!data.path().join("crewd.db").exists(), "不能建出空库");
}

fn add_crew(data: &Path, id: &str) {
    let s = crewd_store::Store::open_data_dir(data).unwrap();
    crewd_store::crews::upsert(
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

/// 按标题看机组，不按个数：第一次建机组会懒建总机组（管理机组），个数随之多一。
fn crew_titles(data: &Path) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(
        &ok(crewd(data).args(["rpc", "crew.list"]).output().unwrap()).stdout,
    )
    .unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| c["title"].as_str().unwrap().to_string())
        .collect()
}

/// serve 在跑时：备份照常（自己的只读连接）、检查照常；恢复被数据目录锁挡住。
/// 停掉之后恢复，再起服务看到的是备份时的数据，原来的库挪在一边。
#[test]
fn backup_while_serving_then_restore_after_stopping() {
    let data = tempfile::tempdir().unwrap();
    add_crew(data.path(), "a");
    let d = start(data.path());
    // 启动时的定时拍：库里有数据、还没有任何定时备份 → 拍一份。
    let backups = data.path().join("backups");
    let deadline = Instant::now() + Duration::from_secs(10);
    while crewd_store::backup::list_auto(&backups).unwrap().is_empty() {
        assert!(Instant::now() < deadline, "启动时没有定时备份");
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = ok(crewd(data.path()).arg("backup").output().unwrap());
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(path.contains("crewd-manual-"), "{path}");
    assert!(Path::new(&path).exists());
    ok(crewd(data.path()).arg("check").output().unwrap());

    let refused = crewd(data.path())
        .args(["restore", "--from"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("另一个 crewd"),
        "{}",
        stderr(&refused)
    );

    ok(crewd(data.path())
        .args([
            "rpc",
            "crew.create",
            r#"{"title":"备份之后建的","workingDirectory":"/tmp/example"}"#,
        ])
        .output()
        .unwrap());
    let before = crew_titles(data.path());
    assert!(before.iter().any(|t| t == "示例 a"), "{before:?}");
    assert!(before.iter().any(|t| t == "备份之后建的"), "{before:?}");
    drop(d);

    ok(crewd(data.path())
        .args(["restore", "--from"])
        .arg(&path)
        .output()
        .unwrap());
    let names: Vec<String> = std::fs::read_dir(data.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("crewd.db.replaced-")),
        "{names:?}"
    );
    let _d = start(data.path());
    let after = crew_titles(data.path());
    assert!(after.iter().any(|t| t == "示例 a"), "{after:?}");
    assert!(
        !after.iter().any(|t| t == "备份之后建的"),
        "恢复的是备份时的数据：{after:?}"
    );
}

#[test]
fn check_fails_loudly_on_a_damaged_db() {
    let data = tempfile::tempdir().unwrap();
    add_crew(data.path(), "a");
    {
        let s = crewd_store::Store::open_data_dir(data.path()).unwrap();
        crewd_store::testing::violate_a_check(s.conn());
    }
    let out = crewd(data.path()).arg("check").output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("CHECK"), "{}", stderr(&out));
    assert!(data.path().join("crewd.db").exists(), "只报告，不删");
}
