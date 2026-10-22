//! T11 actual socket + Core/DbPort + synthetic executable in a real PTY.
mod support {
    pub mod t11;
}
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use support::t11::*;
async fn start(
    f: &Fixture,
    mode: &str,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    Client,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(
        f.data.clone(),
        f.options(mode),
        async {
            let _ = rx.await;
        },
    ));
    let c = tokio::select! {
        outcome = &mut server => panic!("serve exited before socket readiness: {outcome:?}"),
        client = Client::connect(&f.data) => client,
    };
    (tx, server, c)
}
async fn stop(
    tx: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    f: &Fixture,
) {
    tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(registry(&f.data).is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_sessions_start_wake_stop_and_refuse_unqualified_history_resume() {
    let f = Fixture::new();
    let (tx, server, mut c) = start(&f, "ready").await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic managed crew","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let cid = crew["id"].as_str().unwrap();
    let captain=c.call("session.start",json!({"crewId":cid,"role":"captain","executor":"claude","brief":"SYNTHETIC_CAPTAIN","isolation":false})).await.unwrap();
    let capid = captain["id"].as_str().unwrap();
    let worker=c.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"SYNTHETIC_BETA","isolation":false})).await.unwrap();
    let sid = worker["id"].as_str().unwrap();
    wait("managed initial submit", || f.count("SYNTHETIC_BETA") == 1).await;
    wait("actual initial idle", || {
        std::fs::read_to_string(f.log.with_extension("hooks.jsonl"))
            .unwrap_or_default()
            .contains("Notification")
    })
    .await;
    c.call("message.post",json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_WAKE","mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
    wait("fresh managed wake", || f.count("SYNTHETIC_WAKE") == 1).await;
    c.call(
        "session.stop",
        json!({"sessionId":sid,"reason":"synthetic"}),
    )
    .await
    .unwrap();
    c.stopped(sid).await;
    c.call("message.post",json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_MUST_NOT_RESUME","mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
    assert_eq!(
        c.call("session.live", json!({"sessionId":sid}))
            .await
            .unwrap()["running"],
        false
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let queued = c.call("wake.list", json!({"sessionId":sid})).await.unwrap();
        if queued["wakes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["state"] == "pending" && w["attempts"] == 0)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "new stopped wake was not durably queued: {queued}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let err = c
        .call("session.resume", json!({"sessionId":sid}))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("history/resume unavailable"),
        "{err}"
    );
    c.call(
        "session.stop",
        json!({"sessionId":capid,"reason":"synthetic cleanup"}),
    )
    .await
    .unwrap();
    c.stopped(capid).await;
    assert!(registry(&f.data).is_empty());
    assert_eq!(f.count("SYNTHETIC_MUST_NOT_RESUME"), 0);
    stop(tx, server, &f).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolated_start_creates_worktree_and_failed_start_leaves_nothing() {
    let f = Fixture::new();
    let repo = f.root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    // Git reports the physical repository root (e.g. /private/var on macOS).
    let repo = repo.canonicalize().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        let o = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", CHILD_PATH)
            .env("HOME", &f.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    let (tx, server, mut c) = start(&f, "ready").await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic repo","workingDirectory":repo}),
        )
        .await
        .unwrap();
    let cid = crew["id"].as_str().unwrap();
    let shared=c.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"SYNTHETIC_SHARED","isolation":false})).await.unwrap();
    assert!(shared.get("worktree").is_none_or(|w| w.is_null()));
    let iso=c.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"merge-task SYNTHETIC_ISOLATED","isolation":true})).await.unwrap();
    let wt = std::path::PathBuf::from(iso["worktree"].as_str().unwrap());
    assert!(
        wt.is_dir()
            && wt
                .canonicalize()
                .unwrap()
                .starts_with(repo.join(".pendingcrew/worktrees")),
        "{wt:?}"
    );
    wait("isolated submission", || f.count("SYNTHETIC_ISOLATED") == 1).await;
    let missing = c
        .call(
            "session.start",
            json!({"crewId":cid,"role":"worker","executor":"claude","brief":"SYNTHETIC_MISSING"}),
        )
        .await
        .unwrap_err();
    assert_eq!(missing["code"], -32602);
    let plain = c
        .call(
            "crew.create",
            json!({"title":"Synthetic plain","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let pid = plain["id"].as_str().unwrap();
    let before = c.call("session.list", json!({"crewId":pid})).await.unwrap();
    assert!(c.call("session.start",json!({"crewId":pid,"role":"worker","executor":"claude","brief":"SYNTHETIC_FAILED","isolation":true})).await.is_err());
    assert_eq!(
        c.call("session.list", json!({"crewId":pid})).await.unwrap(),
        before
    );
    assert_eq!(f.count("SYNTHETIC_FAILED"), 0);
    assert!(!f.work.join(".pendingcrew").exists());
    stop(tx, server, &f).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_files_live_outside_the_data_dir() {
    let f = Fixture::new();
    let (tx, server, mut c) = start(&f, "ready").await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic runtime","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"SYNTHETIC_RUNTIME","isolation":false})).await.unwrap();
    wait("runtime submission", || f.count("SYNTHETIC_RUNTIME") == 1).await;
    assert!(!f.data.join("run").exists());
    assert_eq!(
        std::fs::metadata(&f.runtime).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let files: Vec<_> = std::fs::read_dir(&f.runtime)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(!files.is_empty(), "actual launch must create runtime files");
    for launch in files {
        assert!(launch.starts_with(&f.runtime) && !launch.starts_with(&f.data));
        assert_eq!(
            std::fs::metadata(&launch).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in ["agent-token", "mcp.json", "settings.json"] {
            let file = launch.join(name);
            assert!(
                file.is_file(),
                "missing actual launch artifact: {}",
                file.display()
            );
            assert_eq!(
                std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    stop(tx, server, &f).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_claude_version_is_refused_before_a_body_menu_or_native_spawn() {
    let f = Fixture::new();
    let mut opts = f.options("decision");
    opts.orchestra
        .as_mut()
        .unwrap()
        .claude_env
        .insert("FAKE_CLAUDE_VERSION".into(), "2.1.294 (Claude Code)".into());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
        let _ = rx.await;
    }));
    let mut c = Client::connect(&f.data).await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic unqualified menu","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let started=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"SYNTHETIC_NEVER_LEGACY_FALLBACK","isolation":false})).await.unwrap();
    c.stopped(started["id"].as_str().unwrap()).await;
    assert_eq!(f.count("SYNTHETIC_NEVER_LEGACY_FALLBACK"), 0);
    assert!(registry(&f.data).is_empty());
    assert!(!f.log.with_extension("hooks.jsonl").exists());
    assert!(
        std::fs::read_dir(&f.runtime).unwrap().next().is_none(),
        "no launch artifacts before version qualification"
    );
    stop(tx, server, &f).await;
}

// The existing real Core/socket/fake-PTY seam, with its exact generated helper
// hooks. Native fields here are synthetic and never qualify a real provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_hook_request_uses_original_entry_waits_for_real_callbacks_and_stops_without_replay()
 {
    let f = Fixture::new();
    let mut opts = f.options("managed-hooks");
    opts.orchestra
        .as_mut()
        .unwrap()
        .claude_env
        .insert("FAKE_CLAUDE_VERSION".into(), "2.1.301 (Claude Code)".into());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
        let _ = rx.await;
    }));
    let mut c = Client::connect(&f.data).await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic managed hook crew","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let cid = crew["id"].as_str().unwrap();
    let started=c.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"MANAGED_ORIGINAL_EXACT_TASK","isolation":false})).await.unwrap();
    let sid = started["id"].as_str().unwrap();
    let callbacks = f.log.with_extension("hooks.jsonl");
    wait("actual generated idle callback", || {
        std::fs::read_to_string(&callbacks)
            .unwrap_or_default()
            .contains("Notification")
    })
    .await;
    let live = c
        .call("session.live", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(
        live["working"], false,
        "only actual same-prompt idle permits waiting: {live}"
    );
    assert_eq!(
        live["running"], true,
        "Stop hook never kills the native runtime"
    );
    assert_eq!(f.count("MANAGED_ORIGINAL_EXACT_TASK"), 1);
    let db = rusqlite::Connection::open_with_flags(
        f.data.join(crewd_store::DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (phase,prompt,turn,body):(String,Option<String>,Option<String>,Vec<u8>)=db.query_row("SELECT r.phase,r.prompt_id,r.turn_id,i.body FROM native_execution_receipts r JOIN native_execution_inputs i USING(request_id)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
    assert_eq!(phase, "completed");
    assert!(prompt.is_some());
    assert_eq!(
        turn, None,
        "managed waiting cannot fabricate a native turnId"
    );
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("\nMANAGED_ORIGINAL_EXACT_TASK\n")
    );
    let lines = std::fs::read_to_string(&callbacks).unwrap();
    assert!(
        lines.lines().next().unwrap().contains("SessionStart"),
        "guard installation precedes the first body"
    );
    assert!(
        lines.contains("UserPromptSubmit")
            && lines.contains("Stop")
            && lines.contains("Notification")
    );
    c.call(
        "session.stop",
        json!({"sessionId":sid,"reason":"synthetic managed stop"}),
    )
    .await
    .unwrap();
    c.stopped(sid).await;
    assert_eq!(f.count("MANAGED_ORIGINAL_EXACT_TASK"), 1);
    stop(tx, server, &f).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_hook_faults_never_promote_screen_idle_or_replay_unknown_inputs() {
    for case in [
        "missing-prompt",
        "subagent",
        "extra-submit",
        "stop-failure",
        "session-end",
        "activity-after-stop",
        "activity-after-idle",
        "failure-after-idle",
        "foreign-idle",
        "stop-only",
        "multiple-stop",
    ] {
        let f = Fixture::new();
        let mut opts = f.options("managed-hooks");
        let env = &mut opts.orchestra.as_mut().unwrap().claude_env;
        env.insert("FAKE_CLAUDE_VERSION".into(), "2.1.301 (Claude Code)".into());
        env.insert("FAKE_CLAUDE_MANAGED_CASE".into(), case.into());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
            let _ = rx.await;
        }));
        let mut c = Client::connect(&f.data).await;
        let crew = c
            .call(
                "crew.create",
                json!({"title":"Synthetic fault","workingDirectory":f.work}),
            )
            .await
            .unwrap();
        let started=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"MANAGED_FAULT_ORIGINAL","isolation":false})).await.unwrap();
        let sid = started["id"].as_str().unwrap();
        let callbacks = f.log.with_extension("hooks.jsonl");
        let last = match case {
            "missing-prompt" | "subagent" | "extra-submit" => "UserPromptSubmit",
            "stop-failure" => "StopFailure",
            "session-end" => "SessionEnd",
            "stop-only" => "Stop",
            _ => "Notification",
        };
        wait(case, || {
            std::fs::read_to_string(&callbacks)
                .unwrap_or_default()
                .contains(last)
        })
        .await;
        let db = rusqlite::Connection::open_with_flags(
            f.data.join(crewd_store::DB_FILE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let expected = if case == "multiple-stop" {
            "completed"
        } else if case == "stop-only" {
            "content_accepted"
        } else {
            "unknown"
        };
        wait("durable fault classification", || {
            db.query_row("SELECT phase FROM native_execution_receipts", [], |r| {
                r.get::<_, String>(0)
            })
            .is_ok_and(|p| p == expected)
        })
        .await;
        let live = c
            .call("session.live", json!({"sessionId":sid}))
            .await
            .unwrap();
        assert_eq!(
            live["running"], true,
            "{case}: Stop/Unknown cannot kill runtime"
        );
        assert_eq!(
            live["working"],
            !matches!(
                case,
                "multiple-stop"
                    | "missing-prompt"
                    | "subagent"
                    | "activity-after-idle"
                    | "failure-after-idle"
            ),
            "{case}: screen quiet is not waiting proof: {live}"
        );
        assert_eq!(
            f.count("MANAGED_FAULT_ORIGINAL"),
            usize::from(!matches!(case, "missing-prompt" | "subagent")),
            "{case}"
        );
        c.call(
            "session.stop",
            json!({"sessionId":sid,"reason":"synthetic fault stop"}),
        )
        .await
        .unwrap();
        c.stopped(sid).await;
        let settled: String = db
            .query_row("SELECT phase FROM native_execution_receipts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            settled,
            if case == "multiple-stop" {
                "completed"
            } else {
                "unknown"
            },
            "{case}"
        );
        stop(tx, server, &f).await;
        if case == "stop-only" {
            let mut opts = f.options("managed-hooks");
            opts.orchestra
                .as_mut()
                .unwrap()
                .claude_env
                .insert("FAKE_CLAUDE_VERSION".into(), "2.1.301 (Claude Code)".into());
            let (tx, rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
                let _ = rx.await;
            }));
            let mut reopened = Client::connect(&f.data).await;
            assert_eq!(
                reopened
                    .call("session.live", json!({"sessionId":sid}))
                    .await
                    .unwrap()["running"],
                false
            );
            assert!(
                registry(&f.data).is_empty(),
                "reopening does not recreate native dispatch authority"
            );
            assert_eq!(
                f.count("MANAGED_FAULT_ORIGINAL"),
                1,
                "old Unknown input never replays"
            );
            assert_eq!(
                db.query_row("SELECT phase FROM native_execution_receipts", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
                "unknown"
            );
            stop(tx, server, &f).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_missing_guard_or_durable_admission_never_sends_the_first_body() {
    for fault in ["skip-start", "db-admission"] {
        let f = Fixture::new();
        let mut opts = f.options("managed-hooks");
        let env = &mut opts.orchestra.as_mut().unwrap().claude_env;
        env.insert("FAKE_CLAUDE_VERSION".into(), "2.1.301 (Claude Code)".into());
        env.insert("FAKE_CLAUDE_MANAGED_CASE".into(), fault.into());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
            let _ = rx.await;
        }));
        let mut c = Client::connect(&f.data).await;
        if fault == "db-admission" {
            let db = rusqlite::Connection::open(f.data.join(crewd_store::DB_FILE)).unwrap();
            db.execute_batch("CREATE TRIGGER synthetic_admission_failure BEFORE INSERT ON native_execution_receipts BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;").unwrap();
        }
        let crew = c
            .call(
                "crew.create",
                json!({"title":"Synthetic guarded admission","workingDirectory":f.work}),
            )
            .await
            .unwrap();
        let started=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"SYNTHETIC_NEVER_BEFORE_DURABLE","isolation":false})).await;
        if fault == "skip-start" {
            // Existing session.start admits its session before asynchronous launch.
            if let Ok(started) = started {
                c.stopped(started["id"].as_str().unwrap()).await;
            }
        } else {
            let started = started.unwrap();
            let sid = started["id"].as_str().unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                let live = c
                    .call("session.live", json!({"sessionId":sid}))
                    .await
                    .unwrap();
                if live["health"]["detail"]
                    .as_str()
                    .is_some_and(|s| s.contains("Unknown"))
                {
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline, "{live}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        assert_eq!(f.count("SYNTHETIC_NEVER_BEFORE_DURABLE"), 0);
        assert!(
            !std::fs::read_to_string(f.log.with_extension("hooks.jsonl"))
                .unwrap_or_default()
                .contains("UserPromptSubmit")
        );
        stop(tx, server, &f).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_manual_custody_allows_only_a_current_human_lease_and_never_auto_replays() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let f = Fixture::new();
    let mut opts = f.options("managed-hooks");
    opts.orchestra
        .as_mut()
        .unwrap()
        .claude_env
        .insert("FAKE_CLAUDE_VERSION".into(), "2.1.301 (Claude Code)".into());
    let trace = f.root.path().join("human-consumption.trace");
    opts.orchestra
        .as_mut()
        .unwrap()
        .claude_env
        .insert("FAKE_CLAUDE_TRACE".into(), trace.display().to_string());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
        let _ = rx.await;
    }));
    let mut c = Client::connect(&f.data).await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic human custody","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let started=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"SYNTHETIC_MANAGED_BEFORE_HUMAN","isolation":false})).await.unwrap();
    let sid = started["id"].as_str().unwrap();
    wait("actual waiting before handoff", || {
        std::fs::read_to_string(f.log.with_extension("hooks.jsonl"))
            .unwrap_or_default()
            .contains("Notification")
    })
    .await;
    let hello = c.call("system.hello", json!({})).await.unwrap();
    let identity = &hello["runtimeIdentity"];
    let view = c
        .call("session.terminal.inspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    let request = json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.terminal.claim","params":{"target":view["target"]}}});
    let claim = c.call("command.submit", request).await.unwrap();
    assert_eq!(claim["phase"], "confirmed", "{claim}");
    let mut target = claim["result"]["target"].clone();
    // Match the original fixture's actual paste/clear-paste/Enter protocol.
    // Physical write receipts alone never count as application consumption.
    for bytes in [b"SYNTHETIC_HUMAN_ONLY".as_slice(), b" ", b"\x7f", b"\r"] {
        let before = std::fs::read_to_string(&trace)
            .unwrap_or_default()
            .matches("consume-result")
            .count();
        let write = json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.terminal.write","params":{"target":target,"leaseId":claim["result"]["leaseId"],"bytesBase64":STANDARD.encode(bytes)}}});
        let receipt = c.call("command.submit", write).await.unwrap();
        assert_eq!(receipt["phase"], "written", "{receipt}");
        target = receipt["result"]["target"].clone();
        wait("actual native chunk consumption", || {
            std::fs::read_to_string(&trace)
                .unwrap_or_default()
                .matches("consume-result")
                .count()
                > before
        })
        .await;
    }
    wait("human native prompt through actual lease", || {
        f.count("SYNTHETIC_HUMAN_ONLY") == 1
    })
    .await;
    assert_eq!(f.count("SYNTHETIC_MANAGED_BEFORE_HUMAN"), 1);
    let db = rusqlite::Connection::open_with_flags(
        f.data.join(crewd_store::DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM native_execution_receipts", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1,
        "Human raw input never fabricates another managed receipt"
    );
    let release = json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.terminal.release","params":{"target":target,"leaseId":claim["result"]["leaseId"]}}});
    assert_eq!(
        c.call("command.submit", release).await.unwrap()["phase"],
        "confirmed"
    );
    let native: String = db
        .query_row(
            "SELECT agent_session_id FROM sessions WHERE id=?1",
            [sid],
            |r| r.get(0),
        )
        .unwrap();
    let launch = std::fs::read_dir(&f.runtime)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(launch.join("settings.json")).unwrap()).unwrap();
    let mut child = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(
            settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap(),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    use tokio::io::AsyncWriteExt;
    child.stdin.take().unwrap().write_all(json!({"session_id":native,"hook_event_name":"UserPromptSubmit","prompt_id":uuid::Uuid::new_v4().to_string(),"prompt":"SYNTHETIC_DENIED_AFTER_RELEASE"}).to_string().as_bytes()).await.unwrap();
    let denied = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        denied.status.code(),
        Some(2),
        "historical manual floor cannot authorize a new native prompt"
    );
    assert_eq!(f.count("SYNTHETIC_DENIED_AFTER_RELEASE"), 0);
    stop(tx, server, &f).await;
}
