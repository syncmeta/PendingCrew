//! Actual P-owned D serve + R rpc siblings, real driver Enrollment, synthetic helpers only.
#![cfg(feature = "testkit")]
#[path = "../../server/tests/support/t11.rs"]
mod t11;
use crewd_exec::registry::{self, ParentProbe, ProbeResult, ProcessIdentity};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use t11::{CHILD_PATH, Fixture, artifact};
const CAP: Duration = Duration::from_secs(15);
struct OwnedChild {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}
impl OwnedChild {
    fn wait(&mut self) -> io::Result<ExitStatus> {
        let until = Instant::now() + CAP;
        loop {
            if let Some(s) = self.child.try_wait()? {
                return Ok(s);
            }
            if Instant::now() >= until {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "owned subprocess wait",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn json(&self) -> io::Result<Value> {
        serde_json::from_slice(&std::fs::read(&self.stdout)?).map_err(io::Error::other)
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            eprintln!(
                "S1 owned subprocess fallback cleanup pid={}",
                self.child.id()
            );
            let _ = self.child.kill();
            let _ = self.wait();
        }
    }
}
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
fn spawn(mut c: Command, f: &Fixture, tag: &str) -> io::Result<OwnedChild> {
    let stdout = f.root.path().join(format!("{tag}.stdout"));
    let stderr = f.root.path().join(format!("{tag}.stderr"));
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stdout)?;
    let err = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stderr)?;
    let child = c.stdin(Stdio::null()).stdout(out).stderr(err).spawn()?;
    Ok(OwnedChild {
        child,
        stdout,
        stderr,
    })
}
fn rpc(f: &Fixture, tag: &str, method: &str, params: Value) -> io::Result<OwnedChild> {
    let mut c = command(f);
    c.args(["rpc", method, &params.to_string()]);
    spawn(c, f, tag)
}
fn rpc_ok(f: &Fixture, tag: &str, method: &str, params: Value) -> io::Result<Value> {
    let mut c = rpc(f, tag, method, params)?;
    let status = c.wait()?;
    if !status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&std::fs::read(&c.stderr)?).to_string(),
        ));
    }
    c.json()
}
fn wait_file(p: &Path) -> io::Result<()> {
    let until = Instant::now() + CAP;
    while !p.exists() {
        if Instant::now() >= until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("owned receipt missing {}", p.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn read_json(p: &Path) -> io::Result<Value> {
    serde_json::from_slice(&std::fs::read(p)?).map_err(io::Error::other)
}
fn pending_path(control: &Path, pid: u32, method: &str) -> PathBuf {
    let tag = method
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    control.join(format!("pending-{pid}-{tag}.json"))
}
fn same_birth(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
fn birth_gone(old: &ProcessIdentity) -> bool {
    matches!(registry::probe(old.pid), ProbeResult::Missing)
        || matches!(registry::probe(old.pid),ProbeResult::Found(now) if !same_birth(old,&now))
}
fn rows(f: &Fixture) -> io::Result<(i64, i64)> {
    // Disposable read-only observation of the live fixture, no Store/open/migration path.
    let c = rusqlite::Connection::open_with_flags(
        f.data.join(crewd_store::DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(io::Error::other)?;
    let crews = c
        .query_row(
            "SELECT count(*) FROM crews WHERE title='S1_LITERAL_PENDING_ONCE'",
            [],
            |r| r.get(0),
        )
        .map_err(io::Error::other)?;
    let messages=c.query_row("SELECT count(*) FROM messages WHERE text LIKE 'S1_SYNTHETIC_%' OR text='S1_AFTER_ATTACH'",[],|r|r.get(0)).map_err(io::Error::other)?;
    Ok((crews, messages))
}
struct Containment {
    control: PathBuf,
    fake: PathBuf,
    birth: Option<ProcessIdentity>,
}
impl Drop for Containment {
    fn drop(&mut self) {
        let _ = std::fs::write(self.control.join("release"), b"");
        let _ = std::fs::write(self.fake.join("finish"), b"");
        let old = self.birth.clone().or_else(|| {
            read_json(&self.control.join("held-birth.json"))
                .ok()
                .and_then(|v| serde_json::from_value(v["identity"].clone()).ok())
        });
        if let Some(old) = old
            && old.pid > 1
            && old.pgid == old.pid
            && old.pgid != unsafe { libc::getpgrp() }
            && matches!(registry::probe(old.pid),ProbeResult::Found(now) if same_birth(&old,&now)&&old.pgid==now.pgid)
        {
            eprintln!(
                "S1 recorded owned birth still live; exact group containment fallback pid={}",
                old.pid
            );
            // The same residual probe→signal race as the existing owned registry reaper.
            let _ = unsafe { libc::kill(-old.pgid, libc::SIGKILL) };
        }
    }
}
#[test]
fn s1_literal_rpc_sibling_waits_real_driver_enrollment_and_pending_managed_peer_stays_denied() {
    let f = Fixture::new();
    let control = f.root.path().join("enrollment");
    let fake = f.root.path().join("s1-peer");
    std::fs::create_dir(&control).unwrap();
    std::fs::create_dir(&fake).unwrap();
    let mut containment = Containment {
        control: control.clone(),
        fake: fake.clone(),
        birth: None,
    };
    let mut c = command(&f);
    c.env("CREWD_S1_ENROLLMENT_CONTROL_DIR", &control)
        .args([
            "serve",
            "--claude-env",
            "FAKE_CLAUDE_MODE=s1-peer",
            "--claude-env",
        ])
        .arg(format!("S1_PEER_FIXTURE_DIR={}", fake.display()))
        .arg("--claude-env")
        .arg(format!("FAKE_CLAUDE_LOG={}", f.log.display()))
        .args(["--claude-env", "FAKE_CLAUDE_VERSION=2.1.301 (Claude Code)"]);
    let mut daemon = spawn(c, &f, "daemon").unwrap();
    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<_> {
        wait_file(&control.join("configured.json"))?;
        wait_file(&f.data.join("crewd.sock"))?;
        let parent = std::process::id() as i32;
        let daemon_parent = registry::parent(daemon.child.id() as i32);
        let crew = rpc_ok(
            &f,
            "initial-crew",
            "crew.create",
            json!({"title":"S1 disposable","workingDirectory":f.work}),
        )?;
        let channel = format!(
            "{}/lobby",
            crew["id"]
                .as_str()
                .ok_or_else(|| io::Error::other("crew id"))?
        );
        let mut start = rpc(
            &f,
            "session-start",
            "session.start",
            json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"S1 literal enrollment","isolation":false}),
        )?;
        wait_file(&control.join("held-birth.json"))?;
        let held = read_json(&control.join("held-birth.json"))?;
        let birth: ProcessIdentity =
            serde_json::from_value(held["identity"].clone()).map_err(io::Error::other)?;
        containment.birth = Some(birth.clone());
        let managed_parent = registry::parent(birth.pid);
        std::fs::write(
            fake.join("requests.json"),
            serde_json::to_vec(&json!({"socket":f.data.join("crewd.sock"),"channel":channel}))
                .map_err(io::Error::other)?,
        )?;
        std::fs::write(fake.join("go"), b"")?;
        // A real generated helper must remain lawful before the held root's first human route.
        wait_file(&fake.join("helper.positive.json"))?;
        let pending_managed = pending_path(&control, birth.pid as u32, "message.post");
        wait_file(&pending_managed)?;
        let mut human = rpc(
            &f,
            "pending-human",
            "crew.create",
            json!({"title":"S1_LITERAL_PENDING_ONCE","workingDirectory":f.work}),
        )?;
        let pending_human = pending_path(&control, human.child.id(), "crew.create");
        wait_file(&pending_human)?;
        let managed_receipt = read_json(&pending_managed)?;
        let human_receipt = read_json(&pending_human)?;
        let before_rows = rows(&f)?;
        let human_before_release = human.child.try_wait()?;
        // Release solely after true parsed pending receipts, never a timer/notification as authority.
        std::fs::write(control.join("release"), b"")?;
        let human_status = human.wait()?;
        let human_reply = human.json();
        let start_status = start.wait()?;
        let session = start.json()?;
        let sid = session["id"]
            .as_str()
            .ok_or_else(|| io::Error::other("session id"))?
            .to_string();
        wait_file(&fake.join("root.responses.json"))?;
        wait_file(&fake.join("child.responses.json"))?;
        wait_file(&fake.join("child.exit.json"))?;
        let root = read_json(&fake.join("root.responses.json"))?;
        let child = read_json(&fake.join("child.responses.json"))?;
        let helper = read_json(&fake.join("helper.positive.json"))?;
        let child_exit = read_json(&fake.join("child.exit.json"))?;
        let after_rows = rows(&f)?;
        std::fs::write(fake.join("finish"), b"")?;
        let stopped = rpc_ok(
            &f,
            "session-stop",
            "session.stop",
            json!({"sessionId":sid,"reason":"S1 controlled literal cleanup"}),
        );
        Ok((
            parent,
            daemon_parent,
            managed_parent,
            held,
            birth,
            human_receipt,
            managed_receipt,
            before_rows,
            human_before_release,
            human_status,
            human_reply,
            start_status,
            root,
            child,
            helper,
            child_exit,
            after_rows,
            stopped,
        ))
    }));
    // Every observation failure releases the true driver before stopping the owned daemon.
    let release = std::fs::write(control.join("release"), b"");
    let finish = std::fs::write(fake.join("finish"), b"");
    let term = unsafe { libc::kill(daemon.child.id() as i32, libc::SIGTERM) };
    let closed = daemon.wait();
    let normal = matches!(&closed,Ok(status) if status.success());
    if !normal {
        let _ = daemon.child.kill();
        let _ = daemon.wait();
    }
    let registry_after =
        crewd_exec::registry::ProcessRegistry::load(&f.data.join("exec-registry.json"))
            .map(|r| r.entries());
    let socket_gone = !f.data.join("crewd.sock").exists();
    let actual_birth_gone = containment.birth.as_ref().is_some_and(birth_gone);
    drop(daemon);
    match observed {
        Err(p) => {
            if !normal {
                eprintln!(
                    "S1 literal daemon close failed after original observation panic: {closed:?}"
                );
            }
            drop(containment);
            std::panic::resume_unwind(p)
        }
        Ok(result) => {
            let data = result
                .expect("all actual receipt/setup observations before normal-close assertion");
            assert!(
                release.is_ok() && finish.is_ok() && term == 0 && normal,
                "normal owned daemon shutdown before functional oracles: {closed:?}"
            );
            assert!(
                socket_gone && registry_after.unwrap().is_empty() && actual_birth_gone,
                "owned actor/storage/socket/root closure"
            );
            let (
                parent,
                dp,
                mp,
                held,
                birth,
                hr,
                mr,
                before,
                early,
                hstatus,
                hreply,
                sstatus,
                root,
                child,
                helper,
                childexit,
                after,
                stopped,
            ) = data;
            assert!(
                matches!(dp,ParentProbe::Found{parent:p,..} if p==parent),
                "actual daemon D parent P"
            );
            assert!(
                matches!(mp,ParentProbe::Found{parent:p,..} if p==held["daemonPid"].as_i64().unwrap_or(-1) as i32),
                "actual managed root parent is D"
            );
            assert_eq!(
                hr["currentParent"], parent,
                "actual kernel RPC R sibling parent P"
            );
            assert_eq!(mr["identity"]["pid"], birth.pid);
            assert!(
                held["enrolling"].as_u64().is_some_and(|n| n > 0),
                "real held driver enrollment"
            );
            assert_eq!(
                before,
                (0, 0),
                "no selected Human effects while parsed requests pending"
            );
            assert!(
                early.is_none(),
                "same original literal Human request must wait actual settlement"
            );
            assert!(
                hstatus.success(),
                "literal Human accepted after fresh settlement"
            );
            assert!(hreply.unwrap()["id"].is_string());
            assert!(sstatus.success());
            assert!(stopped.is_ok());
            assert_eq!(
                after,
                (1, 0),
                "exactly one Human commit, no managed post side effects"
            );
            assert_eq!(helper["normal"], true);
            assert_eq!(helper["code"], 0);
            assert!(helper["responses"][0].get("result").is_some());
            assert!(
                helper["responses"][1]["result"]["tools"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty()),
                "actual generated helper tools remain usable while held"
            );
            assert!(
                root[3].get("result").is_some()
                    && root[4]["result"].as_array().is_some_and(|v| !v.is_empty()),
                "actual root attach/tools remain lawful after settlement"
            );
            assert_eq!(childexit["normal"], true);
            assert_eq!(childexit["code"], 0);
            for responses in [&root, &child] {
                for index in [1, 2] {
                    assert_eq!(
                        responses[index]["error"]["code"],
                        crewd_proto::RpcError::FORBIDDEN
                    );
                    assert_eq!(responses[index]["id"], index + 1);
                }
            }
            assert_eq!(
                root[5]["error"]["code"],
                crewd_proto::RpcError::FORBIDDEN,
                "attached managed peer still denied"
            );
            drop(containment);
        }
    }
}

/// The daemon is an independently frozen default-feature normal binary. Only
/// fake_claude is synthetic; its actual stdin log and real registry births are
/// checked alongside durable fact/wake acknowledgement. This is not ACP SDK proof.
#[test]
#[ignore = "explicit immutable normal13 binary and retained synthetic evidence required"]
fn m5_literal_normal_daemon_physical_driver_delivery_stop_restart_no_old_fact() {
    use crewd_collab::{Sender, crews, sessions, work};
    let normal = PathBuf::from(
        std::env::var_os("PCW_M5_NORMAL_CREWD").expect("explicit verified normal13 executable"),
    );
    assert!(normal.is_absolute() && normal.is_file());
    let mut f = Fixture::new();
    f.root.disable_cleanup(true);
    eprintln!(
        "M5 retained physical normal fixture {} binary {}",
        f.root.path().display(),
        normal.display()
    );
    std::fs::create_dir_all(&f.data).unwrap();
    let (captain, owner, plan) = {
        let mut store = crewd_store::Store::open_data_dir(&f.data).unwrap();
        crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            let c = crews::create(
                tx,
                crews::NewCrew {
                    title: "M5 synthetic physical".into(),
                    working_directory: f.work.display().to_string(),
                    ..Default::default()
                },
            )?;
            let cap = sessions::register(
                tx,
                sessions::NewSession {
                    crew_id: c.id.clone(),
                    role: crewd_proto::Role::Captain,
                    executor: "claude".into(),
                    title: Some("M5 captain".into()),
                    brief: Some("M5 synthetic initial context".into()),
                    ..Default::default()
                },
            )?;
            let owner = sessions::register(
                tx,
                sessions::NewSession {
                    crew_id: c.id.clone(),
                    role: crewd_proto::Role::Worker,
                    executor: "claude".into(),
                    title: Some("M5 failed member".into()),
                    brief: Some("M5 synthetic inactive member".into()),
                    ..Default::default()
                },
            )?;
            let p = work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: c.id,
                    title: "PRIVATE_M5_BODY_MUST_NOT_ENTER_REMINDER_a612".into(),
                    owner: Some(owner.id.clone()),
                    ..Default::default()
                },
            )?;
            sessions::set_resume_failed(tx, &owner.id, true)?;
            Ok((cap.id, owner.id, p.id))
        })
        .unwrap()
        .0
    };
    let local_command = || {
        let mut c = Command::new(&normal);
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
    };
    let start_daemon = |tag: &str| {
        let mut c = local_command();
        c.args([
            "serve",
            "--claude-env",
            "FAKE_CLAUDE_MODE=ready",
            "--claude-env",
        ])
        .arg(format!("FAKE_CLAUDE_LOG={}", f.log.display()))
        .arg("--claude-env")
        .arg("FAKE_CLAUDE_FULL_INPUT=1");
        spawn(c, &f, tag).unwrap()
    };
    let public_rpc = |tag: &str, method: &str, params: Value| {
        let mut c = local_command();
        c.args(["rpc", method, &params.to_string()]);
        let mut child = spawn(c, &f, tag).unwrap();
        let status = child.wait().unwrap();
        assert!(
            status.success(),
            "normal publicRPC {method}: {}",
            std::fs::read_to_string(&child.stderr).unwrap()
        );
        child.json().unwrap()
    };
    let observe = || {
        let c = rusqlite::Connection::open_with_flags(
            f.data.join(crewd_store::DB_FILE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut q=c.prepare("SELECT f.effect,f.original_wake_id,w.state,w.attempts FROM work_followup_facts f LEFT JOIN wakes w ON w.id=f.original_wake_id WHERE f.category='owner_lost' AND f.recipient_snapshot=?1 ORDER BY f.id").unwrap();
        q.query_map([&captain], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<u32>>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let mut daemon = start_daemon("m5-normal-daemon");
    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_file(&f.data.join("crewd.sock")).unwrap();
        let until = Instant::now() + Duration::from_secs(60);
        let acknowledged = loop {
            let rows = observe();
            if rows.len() == 1
                && rows[0].0 == "delivered"
                && rows[0].2.as_deref() == Some("delivered")
            {
                break rows;
            }
            assert!(
                Instant::now() < until,
                "actual normal delivery not acknowledged: {rows:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let input = std::fs::read_to_string(&f.log).unwrap();
        assert_eq!(
            input
                .lines()
                .filter(|s| s.contains("deterministicFollowup") && s.contains(&plan))
                .count(),
            1
        );
        assert!(!input.contains("PRIVATE_M5_BODY_MUST_NOT_ENTER_REMINDER_a612"));
        let entries =
            crewd_exec::registry::ProcessRegistry::load(&f.data.join("exec-registry.json"))
                .unwrap()
                .entries();
        assert!(entries.iter().any(|(id, _)| id == &captain));
        assert!(
            !entries.iter().any(|(id, _)| id == &owner),
            "a loss notification must not start/reassign the failed owner"
        );
        let born = entries
            .into_iter()
            .filter(|(id, _)| id == &captain)
            .map(|(_, b)| b)
            .collect::<Vec<_>>();
        assert!(!born.is_empty());
        public_rpc(
            "m5-public-stop",
            "session.stop",
            json!({"sessionId":captain,"reason":"M5 controlled physical stop"}),
        );
        let until = Instant::now() + CAP;
        loop {
            let settled =
                crewd_exec::registry::ProcessRegistry::load(&f.data.join("exec-registry.json"))
                    .unwrap()
                    .entries()
                    .is_empty();
            if born.iter().all(birth_gone) && settled {
                break;
            }
            assert!(
                Instant::now() < until,
                "actual fake provider birth/registry not settled after public Stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        (acknowledged, input, born)
    }));
    let _ = unsafe { libc::kill(daemon.child.id() as i32, libc::SIGTERM) };
    let closed = daemon.wait().unwrap();
    assert!(closed.success());
    assert!(!f.data.join("crewd.sock").exists());
    let (acknowledged, input, born) = observed.unwrap();
    let mut daemon = start_daemon("m5-normal-restart");
    let reopened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_file(&f.data.join("crewd.sock")).unwrap();
        let readback = public_rpc("m5-normal-readback", "work.get", json!({"id":plan}));
        assert_eq!(readback["owner"], owner);
        assert_eq!(observe(), acknowledged);
        assert_eq!(std::fs::read_to_string(&f.log).unwrap(), input);
        assert!(
            crewd_exec::registry::ProcessRegistry::load(&f.data.join("exec-registry.json"))
                .unwrap()
                .entries()
                .is_empty()
        );
        assert!(born.iter().all(birth_gone));
    }));
    let _ = unsafe { libc::kill(daemon.child.id() as i32, libc::SIGTERM) };
    assert!(daemon.wait().unwrap().success());
    assert!(!f.data.join("crewd.sock").exists());
    reopened.unwrap();
    std::fs::write(f.root.path().join("m5-physical-result.json"),serde_json::to_vec_pretty(&json!({"normalBinary":normal,"plan":plan,"captain":captain,"owner":owner,"factReceipt":acknowledged,"physicalBirths":born,"actualSubmissions":1,"oldFactAfterRestart":false,"sourceSeed":"synthetic official collab transaction before normaldaemon (not a real provider failure)","realModel":false,"nativeSDKCompatibility":false})).unwrap()).unwrap();
}
