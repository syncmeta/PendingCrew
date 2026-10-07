//! N4 存储安全的命令行端到端：库丢了拒绝启动、备份、恢复、检查。临时数据目录 + 合成数据。
use std::path::Path;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

fn crewd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.env("CREWD_DATA_DIR", data)
        .env_remove("PENDINGCREW_DATA_DIR");
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
