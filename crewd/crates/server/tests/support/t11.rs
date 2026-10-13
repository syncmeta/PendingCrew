//! Synthetic T11 fixtures. No provider, user shell, credentials or app data.
#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
pub const CHILD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
pub fn artifact(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join(name);
    assert!(
        p.is_file(),
        "build synthetic artifact first: {}",
        p.display()
    );
    p
}
pub struct Fixture {
    pub root: tempfile::TempDir,
    pub data: PathBuf,
    pub work: PathBuf,
    pub runtime: PathBuf,
    pub log: PathBuf,
    pub home: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let p = root.path();
        let data = p.join("data");
        let work = p.join("work");
        let runtime = p.join("runtime");
        let log = p.join("submitted.log");
        let home = p.join("home");
        let cache = p.join("cache");
        let config = p.join("claude-config");
        for d in [&work, &home, &cache, &config] {
            std::fs::create_dir_all(d).unwrap();
        }
        Self {
            root,
            data,
            work,
            runtime,
            log,
            home,
            cache,
            config,
        }
    }
    pub fn child_env(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HOME".into(), self.home.display().to_string()),
            ("SHELL".into(), "/usr/bin/false".into()),
            ("XDG_CACHE_HOME".into(), self.cache.display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".into(),
                self.config.display().to_string(),
            ),
            ("FAKE_CLAUDE_LOG".into(), self.log.display().to_string()),
            ("FAKE_CLAUDE_FULL_INPUT".into(), "1".into()),
        ])
    }
    // Extra fields appear when the workspace unifies the testkit feature.
    #[allow(clippy::needless_update)]
    pub fn options(&self, mode: &str) -> crewd_server::ServeOptions {
        let mut env = self.child_env();
        env.insert("FAKE_CLAUDE_MODE".into(), mode.into());
        crewd_server::ServeOptions {
            orchestra: Some(crewd_server::OrchestraServe {
                codex: Default::default(),
                codex_env: Default::default(),
                claude: crewd_server::orchestra::exec_claude::ClaudeProgram::Path(artifact(
                    "fake_claude",
                )),
                claude_env: env,
                helper: Some(artifact("crewd")),
                claude_projects_dir: Some(self.config.join("projects")),
                runtime_dir: Some(self.runtime.clone()),
                child_path: Some(CHILD_PATH.into()),
                config: Default::default(),
                shutdown_limit: Duration::from_secs(10),
            }),
            ..Default::default()
        }
    }
    pub fn count(&self, needle: &str) -> usize {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .matches(needle)
            .count()
    }
}
pub struct Client {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    w: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}
impl Client {
    pub async fn connect(data: &Path) -> Self {
        let until = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = UnixStream::connect(data.join("crewd.sock")).await {
                let (r, w) = s.into_split();
                return Self {
                    lines: BufReader::new(r).lines(),
                    w,
                    next: 1,
                };
            }
            assert!(tokio::time::Instant::now() < until, "serve did not bind");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next;
        self.next += 1;
        self.w
            .write_all(
                (json!({"id":id,"method":method,"params":params}).to_string() + "\n").as_bytes(),
            )
            .await
            .unwrap();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(15), self.lines.next_line())
                .await
                .expect("RPC timeout")
                .unwrap()
                .expect("EOF before RPC response");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id {
                return if let Some(e) = v.get("error") {
                    Err(e.clone())
                } else {
                    Ok(v["result"].clone())
                };
            }
        }
    }
    pub async fn stopped(&mut self, sid: &str) {
        let until = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if self
                .call("session.live", json!({"sessionId":sid}))
                .await
                .unwrap()["running"]
                == false
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "session failed to stop"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}
pub async fn wait(what: &str, mut yes: impl FnMut() -> bool) {
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    while !yes() {
        assert!(tokio::time::Instant::now() < until, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
pub fn registry(data: &Path) -> Vec<(String, crewd_exec::registry::ProcessIdentity)> {
    crewd_exec::registry::ProcessRegistry::load(&data.join("exec-registry.json"))
        .unwrap()
        .entries()
}

// On an assertion failure, retain the test failure while cleaning only the
// synthetic children recorded beneath this fixture's private data directory.
// Production registry reaping verifies PID/start time/process group identity.
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Ok(registry) =
                crewd_exec::registry::ProcessRegistry::load(&self.data.join("exec-registry.json"))
        {
            let _ = crewd_exec::registry::reap_orphans(&registry);
        }
    }
}
