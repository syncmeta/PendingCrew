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
async fn managed_sessions_owned_history_exact_resume_cut_and_zero_reopen_input() {
    let f = Fixture::new();
    let mut options=f.options("ready");
    options.orchestra.as_mut().unwrap().config.heartbeat_ms=25;
    let(tx,rx)=tokio::sync::oneshot::channel();
    let data=f.data.clone();
    let server=tokio::spawn(crewd_server::serve_with(data,options,async {let _=rx.await;}));
    let mut c=Client::connect(&f.data).await;
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
    let before=c.call("session.claude.inspect",json!({"sessionId":sid})).await.unwrap();
    assert_eq!(before["manualResume"]["eligibility"],"eligible","{before}");
    assert_eq!(before["fullHistoryLoad"],false);
    let original_id=before["manualResume"]["originalAgentSessionId"].clone();
    let history=c.call("session.claude.history",json!({"sessionId":sid,"limit":32})).await.unwrap();
    let task_request=history["tasks"].as_array().unwrap().last().unwrap()["requestId"].as_str().unwrap().to_owned();
    let detail=c.call("session.claude.taskGet",json!({"sessionId":sid,"requestId":task_request})).await.unwrap();
    assert_eq!(detail["originalTask"]["text"],"SYNTHETIC_BETA");
    assert!(detail["observations"].as_array().unwrap().iter().any(|o|o["text"]=="Synthetic response only"),"{detail}");
    let hello=c.call("system.hello",json!({})).await.unwrap();
    let target=c.call("session.controlTarget",json!({"sessionId":sid})).await.unwrap();
    let resume_id=uuid::Uuid::new_v4().to_string();
    let mut receipt=c.call("command.submit",json!({"requestId":resume_id,"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.resume","params":target}})).await.unwrap();
    let until=tokio::time::Instant::now()+Duration::from_secs(12);
    while receipt["phase"]=="dispatched" {
        assert!(tokio::time::Instant::now()<until,"resume receipt did not settle {receipt}");
        tokio::time::sleep(Duration::from_millis(25)).await;
        receipt=c.call("command.lookup",json!({"requestId":resume_id,"dbId":hello["runtimeIdentity"]["dbId"]})).await.unwrap();
    }
    assert_eq!(receipt["phase"],"launched","{receipt}");
    let resumed=c.call("session.claude.inspect",json!({"sessionId":sid})).await.unwrap();
    assert_eq!(resumed["manualResume"]["originalAgentSessionId"],original_id);
    assert_eq!(resumed["manualResume"]["eligibility"],"running");
    assert_eq!(f.count("SYNTHETIC_MUST_NOT_RESUME"),0,"first launch scan must not deliver old pending input");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(f.count("SYNTHETIC_MUST_NOT_RESUME"),0,"subsequent actual heartbeat scans must keep the same cut");
    let old_queue=c.call("wake.list",json!({"sessionId":sid})).await.unwrap();
    assert!(old_queue["wakes"].as_array().unwrap().iter().any(|w|w["state"]=="pending"&&w["attempts"]==0),"{old_queue}");
    let old_ids:Vec<_>=old_queue["wakes"].as_array().unwrap().iter().filter(|w|w["state"]=="pending"&&w["attempts"]==0).map(|w|w["id"].clone()).collect();
    c.call("message.post",json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_FRESH_AFTER_RESUME","mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
    wait("post-cut new task still reaches exact resumed native session",||f.count("SYNTHETIC_FRESH_AFTER_RESUME")==1).await;
    assert_eq!(f.count("SYNTHETIC_MUST_NOT_RESUME"),0,"new automatic context must not smuggle a pre-resume body");
    let until=tokio::time::Instant::now()+Duration::from_secs(10);
    let after_new=loop {
        let rows=c.call("wake.list",json!({"sessionId":sid})).await.unwrap();
        if rows["wakes"].as_array().unwrap().iter().any(|w|!old_ids.contains(&w["id"])&&w["id"].as_i64().unwrap()>old_ids.last().unwrap().as_i64().unwrap()&&w["state"]=="delivered"){break rows}
        assert!(tokio::time::Instant::now()<until,"fresh task did not reach original Delivered ACK: {rows}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    eprintln!("old IDs after fresh task: {old_ids:?}; current rows={after_new}");
    for id in &old_ids {
        let row=after_new["wakes"].as_array().unwrap().iter().find(|w|&w["id"]==id).expect("original pending row retained");
        assert_eq!(row["state"],"pending","original wake cannot be claimed by new task: {row}");
        assert_eq!(row["attempts"],0,"original wake retains attempts: {row}");
    }
    assert_eq!(f.count("SYNTHETIC_BETA"),1,"original brief never accompanies resume");
    c.call("message.post",json!({"channelId":format!("{cid}/lobby"),"text":"SYNTHETIC_EXPLICIT_QUOTE SYNTHETIC_MUST_NOT_RESUME","mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
    wait("Human explicitly quoting old text is not censored",||f.count("SYNTHETIC_EXPLICIT_QUOTE")==1).await;
    assert_eq!(f.count("SYNTHETIC_MUST_NOT_RESUME"),1,"only the explicit new Human quote may contain this text");
    c.call("session.stop",json!({"sessionId":sid,"reason":"synthetic cleanup"})).await.unwrap();
    c.stopped(sid).await;
    c.call(
        "session.stop",
        json!({"sessionId":capid,"reason":"synthetic cleanup"}),
    )
    .await
    .unwrap();
    c.stopped(capid).await;
    assert!(registry(&f.data).is_empty());
    stop(tx, server, &f).await;
    let count=std::fs::read_to_string(&f.log).unwrap();
    let(tx,server,mut c)=start(&f,"ready").await;
    assert!(registry(&f.data).is_empty(),"Core reopen must not spawn Claude");
    let reopened=c.call("session.claude.history",json!({"sessionId":sid,"limit":32})).await.unwrap();
    assert!(reopened["tasks"].as_array().unwrap().len()>=2);
    assert!(reopened["tasks"].as_array().unwrap().iter().all(|t|t["currentRuntime"]==false&&t["waitingObserved"]==false));
    assert_eq!(std::fs::read_to_string(&f.log).unwrap(),count,"history/reopen sends zero new task bytes");
    stop(tx,server,&f).await;
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

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn owned_managed_resume_refusal_and_wrong_source_keep_original_uuid_and_never_submit(){
    for fault in ["refuse","wrong-resume-source"] {
        let f=Fixture::new();let(tx,server,mut c)=start(&f,"ready").await;
        let crew=c.call("crew.create",json!({"title":"Owned fault","workingDirectory":f.work})).await.unwrap();
        let row=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"SYNTHETIC_MANUAL_FAULT_ORIGINAL","isolation":false})).await.unwrap();let sid=row["id"].as_str().unwrap();
        wait("initial managed task",||f.count("SYNTHETIC_MANUAL_FAULT_ORIGINAL")==1).await;
        let original=c.call("session.claude.inspect",json!({"sessionId":sid})).await.unwrap()["manualResume"]["originalAgentSessionId"].clone();
        c.call("session.stop",json!({"sessionId":sid,"reason":"fixture stop"})).await.unwrap();c.stopped(sid).await;stop(tx,server,&f).await;
        let mut options=f.options(if fault=="refuse"{"refuse"}else{"ready"});options.orchestra.as_mut().unwrap().config.heartbeat_ms=25;
        if fault!="refuse"{options.orchestra.as_mut().unwrap().claude_env.insert("FAKE_CLAUDE_MANAGED_CASE".into(),fault.into());}
        let(tx,rx)=tokio::sync::oneshot::channel();let data=f.data.clone();let server=tokio::spawn(crewd_server::serve_with(data,options,async{let _=rx.await;}));let mut c=Client::connect(&f.data).await;
        assert!(registry(&f.data).is_empty());let hello=c.call("system.hello",json!({})).await.unwrap();let target=c.call("session.controlTarget",json!({"sessionId":sid})).await.unwrap();let request=uuid::Uuid::new_v4().to_string();
        let mut receipt=c.call("command.submit",json!({"requestId":request,"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.resume","params":target}})).await.unwrap();
        let until=tokio::time::Instant::now()+Duration::from_secs(15);
        while receipt["phase"]=="dispatched"{assert!(tokio::time::Instant::now()<until,"{fault}: {receipt}");tokio::time::sleep(Duration::from_millis(25)).await;receipt=c.call("command.lookup",json!({"requestId":request,"dbId":hello["runtimeIdentity"]["dbId"]})).await.unwrap();}
        eprintln!("strict resume fault={fault} receipt={receipt}");
        assert!(!["confirmed","launched"].iter().any(|p|receipt["phase"]==*p));
        c.stopped(sid).await;wait("faulted owned native cleanup",||registry(&f.data).is_empty()).await;
        assert_eq!(c.call("session.claude.inspect",json!({"sessionId":sid})).await.unwrap()["manualResume"]["originalAgentSessionId"],original);
        assert_eq!(f.count("SYNTHETIC_MANUAL_FAULT_ORIGINAL"),1);
        c.call("message.post",json!({"channelId":format!("{}/lobby",crew["id"].as_str().unwrap()),"text":"SYNTHETIC_NO_AUTO_AFTER_FAILED_RESUME","mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(registry(&f.data).is_empty());assert_eq!(f.count("SYNTHETIC_NO_AUTO_AFTER_FAILED_RESUME"),0);
        assert_eq!(c.call("command.lookup",json!({"requestId":request,"dbId":hello["runtimeIdentity"]["dbId"]})).await.unwrap(),receipt,"only the original UUID is queried, never resubmitted");
        stop(tx,server,&f).await;
    }
}

#[tokio::test]
async fn owned_managed_history_attached_agent_cannot_enter_any_human_read(){
    use crewd_server::agent::testkit::{Fixture as AgentFixture,RpcClient};
    let fixture=AgentFixture::new().await;let(stop,rx)=tokio::sync::oneshot::channel();let server=fixture.spawn_owned_socket(rx).await;
    let mut c=RpcClient::connect(&fixture.data_dir().join("crewd.sock")).await;
    c.call("agent.attach",json!({"sessionId":fixture.worker,"token":fixture.token(&fixture.worker)})).await.unwrap();
    for method in ["session.claude.inspect","session.claude.history","session.claude.taskGet"] {
        let denied=c.call(method,json!({"sessionId":fixture.worker,"requestId":uuid::Uuid::new_v4().to_string()})).await.unwrap_err();
        assert_eq!(denied.0,crewd_proto::RpcError::FORBIDDEN,"{method}: {denied:?}");
    }
    stop.send(()).unwrap();tokio::time::timeout(Duration::from_secs(10),server).await.unwrap().unwrap();
}
