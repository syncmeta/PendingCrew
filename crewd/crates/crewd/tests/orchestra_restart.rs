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
        Self::start_with_env(f, &[])
    }
    fn start_with_env(f: &Fixture, extra: &[(&str, String)]) -> Self {
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(f.root.path().join("serve-stderr.log"))
            .unwrap();
        let mut cmd = command(f);
        cmd.args(["serve", "--claude-env"])
            .arg(format!("FAKE_CLAUDE_LOG={}", f.log.display()))
            .args(["--claude-env", "FAKE_CLAUDE_FULL_INPUT=1"])
            .args(["--claude-env", "FAKE_CLAUDE_VERSION=2.1.301 (Claude Code)"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr);
        for (key, value) in extra {
            cmd.arg("--claude-env").arg(format!("{key}={value}"));
        }
        let child = cmd.spawn().unwrap();
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

/// The actual parent owns this daemon. A separate `crewd rpc` sibling cannot
/// manufacture the owned-child authority required by public hibernation.
fn owner_rpc(f: &Fixture, method: &str, params: Value) -> Value {
    use std::io::{BufRead, Write};
    let mut socket = std::os::unix::net::UnixStream::connect(f.data.join("crewd.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    writeln!(
        socket,
        "{}",
        json!({"id":1,"method":method,"params":params})
    )
    .unwrap();
    let mut reader = std::io::BufReader::new(socket);
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "owned response must flush before exit"
        );
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["id"] == 1 {
            assert!(frame.get("error").is_none(), "{frame}");
            return frame["result"].clone();
        }
    }
}
fn busy_records(f: &Fixture) -> Vec<Value> {
    let directory = f.root.path().join("active-witness");
    let mut paths: Vec<_> = match std::fs::read_dir(directory) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(error) => panic!("cannot read owned witness: {error}"),
    };
    paths.sort();
    let mut records = vec![];
    for path in paths {
        let bytes = std::fs::read(path).unwrap();
        // The per-generation writer flushes complete JSONL records. Ignore only
        // an in-progress trailing record, never a malformed completed witness.
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        records.extend(
            bytes[..end]
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| {
                    serde_json::from_slice::<Value>(line).expect("complete owned witness JSONL")
                }),
        );
    }
    records
}

#[test]
fn active_owned_hibernate_confirms_physical_exit_and_reopen_never_replays_original_task() {
    use crewd_exec::registry::{
        ProbeResult, ProcessRegistry, probe,
        recovery::{GroupObservation, observe_group},
    };
    let f = Fixture::new();
    let active_env = [
        (
            "FAKE_CLAUDE_C13_GATE",
            f.root.path().join("active-gate").display().to_string(),
        ),
        (
            "FAKE_CLAUDE_C13_WITNESS",
            f.root.path().join("active-witness").display().to_string(),
        ),
    ];
    let mut first = Serve::start_with_env(&f, &active_env);
    let identity = owner_rpc(&f, "system.hello", json!({}))["runtimeIdentity"].clone();
    let crew = rpc(
        &f,
        "crew.create",
        json!({"title":"Active sleep proof","workingDirectory":f.work}),
    )
    .unwrap();
    let row=rpc(&f,"session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","title":"Active worker","brief":"","isolation":false})).unwrap();
    let sid = row["id"].as_str().unwrap().to_owned();
    wait("actual original fake launch", || {
        busy_records(&f).iter().any(|r| r["event"] == "launch")
    });
    let launch = busy_records(&f)
        .into_iter()
        .find(|r| r["event"] == "launch")
        .unwrap();
    assert_eq!(launch["session_id"], sid);
    assert_eq!(
        busy_records(&f)
            .iter()
            .filter(|r| r["event"] == "launch")
            .count(),
        1
    );
    assert_eq!(
        busy_records(&f)
            .iter()
            .filter(|r| r["event"] == "submit")
            .count(),
        0,
        "empty brief must create a worker with no initial task"
    );
    let gate = std::path::PathBuf::from(launch["gate"].as_str().unwrap());
    std::fs::write(gate.join("arm"), b"hold actual authenticated task").unwrap();
    let message_id = "f732b199-e93b-440e-8d40-1580d9fc9c31";
    let message = owner_rpc(
        &f,
        "command.submit",
        json!({"requestId":message_id,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"message.post","params":{"channelId":format!("{}/lobby",crew["id"].as_str().unwrap()),"text":"ACTIVE_SLEEP_ORIGINAL_ONLY","mentions":[{"kind":"session","targetId":sid}]}}}),
    );
    assert_eq!(message["phase"], "committed");
    wait("authenticated busy callback held", || {
        gate.join("busy-written").is_file()
    });
    assert!(!gate.join("released").exists());
    let live = rpc(&f, "session.live", json!({"sessionId":sid})).unwrap();
    assert_eq!(live["running"], true);
    assert_eq!(
        live["working"], true,
        "must hibernate active task, not idle"
    );
    let observed = busy_records(&f);
    assert_eq!(
        observed.iter().filter(|r| r["event"] == "submit").count(),
        1
    );
    let conn = rusqlite::Connection::open_with_flags(
        f.data.join("crewd.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (task,runtime,prompt,phase,body):(String,String,String,String,Vec<u8>)=conn.query_row("SELECT r.request_id,r.runtime_id,r.prompt_id,r.phase,i.body FROM native_execution_receipts r JOIN native_execution_runtimes n USING(runtime_id) JOIN native_execution_inputs i USING(request_id) WHERE n.session_id=?1",[&sid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    assert_eq!(
        phase, "content_accepted",
        "managed activity is not a fabricated native turn"
    );
    assert_eq!(
        std::fs::read_to_string(gate.join("managed-prompt")).unwrap(),
        prompt
    );
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("ACTIVE_SLEEP_ORIGINAL_ONLY")
    );
    drop(conn);
    let old = registry(&f.data);
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].0, sid);
    let birth = old[0].1.clone();
    assert_eq!(launch["pid"], birth.pid);
    assert!(matches!(probe(birth.pid),ProbeResult::Found(actual) if actual==birth));
    let native =
        rpc(&f, "session.get", json!({"sessionId":sid})).unwrap()["agentSessionId"].clone();
    assert!(native.is_string());
    eprintln!(
        "ACTIVE_PROOF session={sid} task={task} runtime={runtime} prompt={prompt} native={native} birth={birth:?} receiptPhase={phase} running=true working=true submits=1"
    );
    let hibernate_id = "5cb86c4e-6c3a-48e2-a7ab-596c26eab063";
    let hibernate_started = Instant::now();
    let receipt = owner_rpc(
        &f,
        "command.submit",
        json!({"requestId":hibernate_id,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"crewd.hibernate","params":{"deadlineSecs":0}}}),
    );
    assert_eq!(receipt["phase"], "confirmed", "{receipt}");
    assert_eq!(receipt["result"]["physicalChildrenExited"], true);
    assert_eq!(receipt["result"]["hibernationRecordPersisted"], true);
    assert!(first.wait_exit().success());
    let hibernate_elapsed = hibernate_started.elapsed();
    assert_old_children_gone(&old);
    wait("original group physically absent", || {
        matches!(observe_group(birth.pgid), GroupObservation::Absent)
    });
    assert_registry_clean(&f);
    let reg = ProcessRegistry::load(&f.data.join(crewd_server::PROCESS_REGISTRY_FILE)).unwrap();
    assert!(reg.authority_snapshot().unwrap().spawned.is_empty());
    assert!(
        !gate.join("released").exists(),
        "normal Stop must terminate held task without releasing its execution gate"
    );
    let store = crewd_store::Store::open_data_dir(&f.data).unwrap();
    let stopped:(String,String,String,Vec<u8>)=store.conn().query_row("SELECT r.runtime_id,r.prompt_id,r.phase,i.body FROM native_execution_receipts r JOIN native_execution_inputs i USING(request_id) WHERE r.request_id=?1",[&task],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(
        stopped,
        (
            runtime.clone(),
            prompt.clone(),
            "unknown".into(),
            body.clone()
        ),
        "interrupted execution cannot become completed"
    );
    drop(store);
    eprintln!(
        "HIBERNATE_PROOF elapsedMs={} request={hibernate_id} phase={} physicalChildrenExited={} hibernationRecordPersisted={} originalBirthGone=true originalGroupAbsent=true registryEmpty=true nativePhase=unknown",
        hibernate_elapsed.as_millis(),
        receipt["phase"],
        receipt["result"]["physicalChildrenExited"],
        receipt["result"]["hibernationRecordPersisted"]
    );
    let before = std::fs::read(&f.log).unwrap();
    let mut reopened = Serve::start_with_env(&f, &active_env);
    assert_eq!(
        rpc(&f, "session.get", json!({"sessionId":sid})).unwrap()["agentSessionId"],
        native
    );
    let recovered = owner_rpc(
        &f,
        "command.lookup",
        json!({"requestId":hibernate_id,"dbId":identity["dbId"]}),
    );
    assert_eq!(recovered["requestId"], receipt["requestId"]);
    assert_eq!(recovered["command"], receipt["command"]);
    assert_eq!(recovered["result"], receipt["result"]);
    assert_eq!(recovered["historical"], true);
    let original_message = owner_rpc(
        &f,
        "command.lookup",
        json!({"requestId":message_id,"dbId":identity["dbId"]}),
    );
    assert_eq!(original_message["command"], message["command"]);
    assert_eq!(original_message["result"], message["result"]);
    // Let ordinary scheduler ticks run; reopening is observational, not a task release.
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(
        rpc(&f, "session.live", json!({"sessionId":sid})).unwrap()["running"],
        false
    );
    assert_eq!(std::fs::read(&f.log).unwrap(), before);
    assert_eq!(busy_records(&f), observed);
    assert!(registry(&f.data).is_empty());
    reopened.term();
    assert_registry_clean(&f);
    assert_storage(&f);
    let store = crewd_store::Store::open_data_dir(&f.data).unwrap();
    let final_task:(String,String,String,Vec<u8>)=store.conn().query_row("SELECT r.runtime_id,r.prompt_id,r.phase,i.body FROM native_execution_receipts r JOIN native_execution_inputs i USING(request_id) WHERE r.request_id=?1",[&task],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(final_task, stopped);
    eprintln!(
        "REOPEN_PROOF originalNativeIdPreserved=true commandHistorical=true originalTaskPreserved=true observationMs=900 newLaunches=0 taskResubmits=0"
    );
}
