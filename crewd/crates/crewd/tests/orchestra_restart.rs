//! Actual synthetic binary lifecycle: owned PIDs only, isolated HOME/cache/config.
#[path = "../../server/tests/support/t11.rs"]
mod t11;
use serde_json::{Value, json};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use t11::*;
fn command(f: &Fixture) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.env_clear()
        .env("PATH", CHILD_PATH)
        .env("HOME", &f.home)
        .env("XDG_CACHE_HOME", &f.cache)
        .env("CLAUDE_CONFIG_DIR", &f.config)
        .env("TMPDIR", f.root.path())
        .env("SHELL", "/usr/bin/false")
        .env("CREWD_CLAUDE", artifact("fake_claude"))
        .env("CREWD_DATA_DIR", &f.data)
        .env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    c
}
fn rpc(f: &Fixture, method: &str, params: Value) -> Result<Value, String> {
    let o = command(f)
        .args(["rpc", method, &params.to_string()])
        .output()
        .unwrap();
    if o.status.success() {
        Ok(serde_json::from_slice(&o.stdout).unwrap())
    } else {
        Err(format!(
            "{}\nsynthetic serve log: {}",
            String::from_utf8_lossy(&o.stderr),
            std::fs::read_to_string(f.root.path().join("serve-stderr.log")).unwrap_or_default()
        ))
    }
}
struct Serve {
    child: Child,
}
impl Serve {
    fn start(f: &Fixture) -> Self {
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(f.root.path().join("serve-stderr.log"))
            .unwrap();
        let child = command(f)
            .args(["serve", "--claude-env"])
            .arg(format!("FAKE_CLAUDE_LOG={}", f.log.display()))
            .args(["--claude-env", "FAKE_CLAUDE_FULL_INPUT=1"])
            .args(["--claude-env", "FAKE_CLAUDE_VERSION=2.1.301 (Claude Code)"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap();
        let mut s = Self { child };
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if rpc(f, "system.hello", Value::Null).is_ok() {
                return s;
            }
            assert!(
                s.child.try_wait().unwrap().is_none(),
                "serve exited: {}",
                std::fs::read_to_string(f.root.path().join("serve-stderr.log")).unwrap()
            );
            assert!(Instant::now() < until, "serve did not start");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < until, "serve failed bounded exit");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn term(&mut self) {
        let s = Command::new("/bin/kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(s.success());
        assert!(self.wait_exit().success());
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        assert!(!self.child.wait().unwrap().success());
    }
}
impl Drop for Serve {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn wait(what: &str, mut yes: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    while !yes() {
        assert!(Instant::now() < until, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn start_pair(f: &Fixture) -> (String, String, String) {
    let crew = rpc(
        f,
        "crew.create",
        json!({"title":"Synthetic lifecycle","workingDirectory":f.work}),
    )
    .unwrap();
    let start = |role: &str, brief: &str| {
        rpc(f,"session.start",json!({"crewId":crew["id"],"role":role,"executor":"claude","brief":brief,"isolation":false})).unwrap()["id"].as_str().unwrap().to_string()
    };
    let cap = start("captain", "SYNTHETIC_ALPHA");
    let worker = start("worker", "SYNTHETIC_BETA");
    wait("initial submissions", || {
        f.count("SYNTHETIC_ALPHA") == 1 && f.count("SYNTHETIC_BETA") == 1
    });
    // Body logging precedes Stop/idle hooks. Only the matched durable receipts
    // establish that both managed turns may accept the next Human task.
    let conn = rusqlite::Connection::open_with_flags(f.data.join("crewd.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    wait("initial managed idle receipts", || conn.query_row(
        "SELECT COUNT(*) FROM native_execution_receipts r JOIN native_execution_runtimes n USING(runtime_id) WHERE n.session_id IN (?1,?2) AND r.phase='completed'",
        rusqlite::params![cap,worker], |r|r.get::<_,i64>(0)).unwrap()==2);

    (crew["id"].as_str().unwrap().to_string(), cap, worker)
}
fn message_ids(f: &Fixture) -> Vec<(i64, String)> {
    // This observer runs while the owned test daemon is live. Store open is a
    // stopped-state recovery/version operation with stable-file preconditions.
    // A normal read-only SQLite connection reads the live committed WAL instead.
    let conn = rusqlite::Connection::open_with_flags(
        f.data.join("crewd.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = conn
        .prepare("SELECT seq,id FROM messages ORDER BY seq")
        .unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
fn assert_storage(f: &Fixture) {
    let store = crewd_store::Store::open_data_dir(&f.data).unwrap();
    let conn = store.conn();
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM wakes WHERE state='inflight'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    // seq is a GLOBAL AUTOINCREMENT, not per-channel numbering. Synthetic fixture
    // never deletes messages: global continuity plus channel order and cursor bounds.
    let mut st = conn
        .prepare("SELECT seq,channel_id FROM messages ORDER BY seq")
        .unwrap();
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut channels = std::collections::BTreeMap::new();
    for (i, (seq, channel)) in rows.iter().enumerate() {
        assert_eq!(*seq, i as i64 + 1, "global seq gap");
        if let Some(previous) = channels.insert(channel, *seq) {
            assert!(previous < *seq);
        }
    }
    assert_eq!(conn.query_row("SELECT count(*) FROM read_cursors c WHERE c.seq > coalesce((SELECT max(m.seq) FROM messages m WHERE m.channel_id=c.channel_id),0)",[],|r|r.get::<_,i64>(0)).unwrap(),0,"cursor beyond channel messages");
}
fn assert_registry_clean(f: &Fixture) {
    assert!(registry(&f.data).is_empty());
    assert!(!f.data.join("crewd.sock").exists());
}
fn assert_old_children_gone(old: &[(String, crewd_exec::registry::ProcessIdentity)]) {
    wait("original fake children reaped", || {
        old.iter().all(
            |(_, identity)| match crewd_exec::registry::probe(identity.pid) {
                crewd_exec::registry::ProbeResult::Missing => true,
                crewd_exec::registry::ProbeResult::Found(now) => now != *identity,
                crewd_exec::registry::ProbeResult::Unreadable(_) => false,
            },
        )
    });
}
fn note(key: &str) -> &'static str {
    crewd_wake::text::get(crewd_wake::text::Lang::Zh, key)
}
#[test]
fn crash_reopen_and_explicit_resume_preserve_old_wakes_without_automatic_replay() {
    let f = Fixture::new();
    let mut first = Serve::start(&f);
    let (cid, cap, worker) = start_pair(&f);
    rpc(&f, "message.post", json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_BEFORE_CRASH","mentions":[{"kind":"session","targetId":worker}]})).unwrap();
    wait("pre-crash message delivered", || {
        f.count("SYNTHETIC_BEFORE_CRASH") == 1
    });
    let before_ids = message_ids(&f);
    assert!(!before_ids.is_empty());
    let old = registry(&f.data);
    assert_eq!(old.len(), 2);
    first.crash();
    {
        let mut store = crewd_store::Store::open_data_dir(&f.data).unwrap();
        let now = crewd_store::now_ms();
        crewd_collab::run(&mut store, now, |tx| {
            // Crash-window durable fixture, not provider-working evidence. Both
            // bits are written in one offline transaction after the serve PID
            // has exited; never introduce an external writer beside live Db.
            crewd_collab::sessions::set_working(tx, &worker, false)?;
            crewd_collab::sessions::set_working(tx, &cap, true)?;
            crewd_collab::wakes::enqueue(
                tx,
                crewd_collab::wakes::WakeRequest {
                    session: worker.clone(),
                    kind: crewd_collab::wakes::WakeKind::Timer,
                    source: crewd_collab::wakes::source::timer("synthetic-restart"),
                    priority: crewd_collab::wakes::WakePriority::Auto,
                    not_before_ms: Some(now + 300),
                    note: Some("SYNTHETIC_TIMER".into()),
                    origin: Some(worker.clone()),
                },
            )?;
            Ok(())
        })
        .unwrap();
    }
    let mut second = Serve::start(&f);
    assert_old_children_gone(&old);
    let prompt = rpc(&f, "restart.pending", Value::Null).unwrap();
    assert_eq!(prompt["reason"], "restart");
    let ids = prompt["sessionIds"].as_array().unwrap();
    assert!(
        ids.contains(&json!(cap)) && ids.contains(&json!(worker)),
        "{prompt}"
    );
    std::thread::sleep(Duration::from_millis(600));
    for sid in [&cap, &worker] {
        assert_eq!(
            rpc(&f, "session.live", json!({"sessionId":sid})).unwrap()["running"],
            false
        );
    }
    assert_eq!(f.count("SYNTHETIC_TIMER"), 0);
    rpc(&f, "restart.answer", json!({"id":prompt["id"],"answer":"later"})).unwrap();
    rpc(&f, "session.resume", json!({"sessionId":worker})).unwrap();
    wait("explicit original session resumed", || rpc(&f,"session.live",json!({"sessionId":worker})).unwrap()["running"] == true);
    assert_eq!(f.count("SYNTHETIC_TIMER"), 0);
    assert_eq!(f.count(note("restart.interrupted")), 0);
    rpc(&f,"message.post",json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_NEW_AFTER_EXPLICIT_RESUME","mentions":[{"kind":"session","targetId":worker}]})).unwrap();
    wait("new Human task reaches explicit resumed session", || f.count("SYNTHETIC_NEW_AFTER_EXPLICIT_RESUME") == 1);
    assert_eq!(f.count("SYNTHETIC_TIMER"), 0);
    assert_eq!(f.count("SYNTHETIC_ALPHA"), 1);
    assert_eq!(f.count("SYNTHETIC_BETA"), 1);
    let after_ids = message_ids(&f);
    assert!(
        before_ids.iter().all(|id| after_ids.contains(id)),
        "restart lost historical message identities"
    );
    // Quiescent storage assertions belong after the owned server exits; a
    // logged body can still have a legitimate in-flight ACK while it is live.
    second.term();
    assert_registry_clean(&f);
    assert_storage(&f);
}
#[test]
fn hibernate_response_and_physical_cleanup_reopen_without_implicit_tasks() {
    let f = Fixture::new();
    let mut first = Serve::start(&f);
    let (cid, cap, worker) = start_pair(&f);
    wait("both sessions actually idle before hibernate probe", || {
        [&cap, &worker].iter().all(|sid| {
            rpc(&f, "session.live", json!({"sessionId":sid})).unwrap()["working"] == false
        })
    });
    let began = Instant::now();
    let response = rpc(&f, "crewd.hibernate", json!({"deadlineSecs":0}))
        .expect("CLIENT must receive flushed final hibernate response");
    assert!(response.is_object(), "{response}");
    assert!(first.wait_exit().success());
    assert!(began.elapsed() < Duration::from_secs(15));
    assert_registry_clean(&f);
    assert_storage(&f);
    let mut second = Serve::start(&f);
    for sid in [&cap, &worker] {
        assert_eq!(rpc(&f,"session.live",json!({"sessionId":sid})).unwrap()["running"],false);
    }
    assert_eq!(f.count("SYNTHETIC_ALPHA"), 1);
    assert_eq!(f.count("SYNTHETIC_BETA"), 1);
    assert_eq!(f.count(note("hibernate.interrupted")), 0);
    rpc(&f,"session.resume",json!({"sessionId":cap})).unwrap();
    rpc(&f,"message.post",json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_NEW_AFTER_HIBERNATE","mentions":[{"kind":"session","targetId":cap}]})).unwrap();
    wait("new Human task after explicit hibernate resume",||f.count("SYNTHETIC_NEW_AFTER_HIBERNATE")==1);
    assert_eq!(f.count(note("hibernate.interrupted")), 0);
    let resumed = registry(&f.data);
    second.term();
    assert_old_children_gone(&resumed);
    assert_registry_clean(&f);
    assert_storage(&f);
}
#[test]
fn sigterm_stops_synthetic_children_and_preserves_storage() {
    let f = Fixture::new();
    let mut serve = Serve::start(&f);
    start_pair(&f);
    let old = registry(&f.data);
    assert_eq!(old.len(), 2);
    serve.term();
    assert_old_children_gone(&old);
    assert_registry_clean(&f);
    assert_storage(&f);
}
