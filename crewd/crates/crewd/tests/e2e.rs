use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

fn crewd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.env("CREWD_DATA_DIR", data)
        .env_remove("PENDINGCREW_DATA_DIR");
    c
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../legacy/tests/fixtures/basic")
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

fn rpc(data: &Path, method: &str, params: &str) -> serde_json::Value {
    let out: Output = crewd(data).args(["rpc", method, params]).output().unwrap();
    assert!(
        out.status.success(),
        "{method}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn import_then_serve_then_rpc() {
    let data = tempfile::tempdir().unwrap();
    let out = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["counts"]["messages"], 3);

    let again = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(!again.status.success(), "已有库时必须拒绝导入");

    let _d = start(data.path());
    let crews = rpc(data.path(), "crew.list", "null");
    assert_eq!(crews.as_array().unwrap().len(), 3);
    let before = rpc(data.path(), "whiteboard.list", r#"{"crewId":"local-aaaa"}"#);
    assert_eq!(before.as_array().unwrap().len(), 3);
    rpc(
        data.path(),
        "whiteboard.post",
        r#"{"crewId":"local-aaaa","text":"切换后第一条 ✅","senderKind":"user"}"#,
    );
    let after = rpc(data.path(), "whiteboard.list", r#"{"crewId":"local-aaaa"}"#);
    assert_eq!(after[3]["text"], "切换后第一条 ✅");

    let second = crewd(data.path()).arg("serve").output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("另一个 crewd"));
}

/// 夹具的可写副本（共享夹具本身不许动）。
fn fixture_copy() -> tempfile::TempDir {
    fn cp(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let p = e.unwrap().path();
            let t = to.join(p.file_name().unwrap());
            if p.is_dir() {
                cp(&p, &t);
            } else {
                std::fs::copy(&p, &t).unwrap();
            }
        }
    }
    let t = tempfile::tempdir().unwrap();
    cp(&fixture(), t.path());
    t
}

fn list_tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(list_tree(&p));
        } else {
            let b = std::fs::read(&p).unwrap();
            out.push((p, b));
        }
    }
    out.sort();
    out
}

#[test]
fn import_into_the_legacy_dir_itself_is_refused_and_writes_nothing() {
    let legacy = fixture_copy();
    let before = list_tree(legacy.path());
    let out = crewd(legacy.path())
        .args(["import", "--from"])
        .arg(legacy.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("重叠"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!legacy.path().join("crewd.db").exists());
    assert!(!legacy.path().join("crewd.lock").exists());
    assert_eq!(list_tree(legacy.path()), before, "旧目录一个字节都不能变");
}

#[test]
fn rpc_error_exits_nonzero() {
    let data = tempfile::tempdir().unwrap();
    let _d = start(data.path());
    let out = crewd(data.path())
        .args(["rpc", "no.such"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("未知方法"));
}

#[test]
fn committed_schema_matches_generated() {
    let generated = Command::new(env!("CARGO_BIN_EXE_crewd"))
        .arg("schema")
        .output()
        .unwrap();
    assert!(generated.status.success());
    let committed = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/crewd-api.schema.json"),
    )
    .expect("缺 crewd/schema/crewd-api.schema.json，运行 `cargo run -p crewd -- schema > schema/crewd-api.schema.json`");
    let a: serde_json::Value = serde_json::from_slice(&generated.stdout).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&committed).unwrap();
    assert_eq!(
        a, b,
        "API 契约变了：重新生成 schema/crewd-api.schema.json 并提交"
    );
}
