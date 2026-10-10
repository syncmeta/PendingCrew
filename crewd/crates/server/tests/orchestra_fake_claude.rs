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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_start_wake_resume_and_stop_through_the_real_driver() {
    let f = Fixture::new();
    let (tx, server, mut c) = start(&f, "ready").await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic crew","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let cid = crew["id"].as_str().unwrap();
    let cap=c.call("session.start",json!({"crewId":cid,"role":"captain","executor":"claude","brief":"SYNTHETIC_ALPHA","isolation":false})).await.unwrap();
    let worker=c.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"SYNTHETIC_BETA","isolation":false})).await.unwrap();
    let sid = worker["id"].as_str().unwrap();
    let capid = cap["id"].as_str().unwrap();
    wait("both initial submissions", || {
        f.count("SYNTHETIC_ALPHA") == 1 && f.count("SYNTHETIC_BETA") == 1
    })
    .await;
    for text in ["SYNTHETIC_WAKE", "SYNTHETIC_RESUME_WAKE"] {
        if text == "SYNTHETIC_RESUME_WAKE" {
            c.call(
                "session.stop",
                json!({"sessionId":sid,"reason":"synthetic"}),
            )
            .await
            .unwrap();
            c.stopped(sid).await;
        }
        c.call("message.post",json!({"channelId":format!("{cid}/lobby"),"text":text,"mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
        wait(text, || f.count(text) == 1).await;
    }
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let wakes = c.call("wake.list", json!({"sessionId":sid})).await.unwrap();
        if wakes["wakes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["state"] != "pending" && w["state"] != "inflight")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "wakes did not settle: {wakes}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    c.call(
        "session.stop",
        json!({"sessionId":capid,"reason":"synthetic"}),
    )
    .await
    .unwrap();
    c.stopped(capid).await;
    let mut c2 = Client::connect(&f.data).await;
    let (a, b) = tokio::join!(
        c.call("session.resume", json!({"sessionId":capid})),
        c2.call("session.resume", json!({"sessionId":capid}))
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "{a:?} {b:?}"
    );
    let e = a.err().or_else(|| b.err()).unwrap();
    assert_eq!(e["code"], -32009);
    assert_eq!(e["data"]["refusal"], "session.already_running");
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_answer_reaches_the_synthetic_ordinary_menu() {
    let f = Fixture::new();
    let (tx, server, mut c) = start(&f, "decision").await;
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Synthetic menu","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let s=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"SYNTHETIC_MENU","isolation":false})).await.unwrap();
    let sid = s["id"].as_str().unwrap();
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    let decision = loop {
        let live = c
            .call("session.live", json!({"sessionId":sid}))
            .await
            .unwrap();
        if live["pendingDecision"].is_object() {
            break live["pendingDecision"].clone();
        }
        assert!(
            tokio::time::Instant::now() < until,
            "menu not observed: {live}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    c.call(
        "session.answer",
        json!({"sessionId":sid,"fingerprint":decision["fingerprint"],"choice":1}),
    )
    .await
    .unwrap();
    wait("menu keys received by fake", || {
        f.count("DECISION-ANSWER:2") == 1
    })
    .await;
    stop(tx, server, &f).await;
}
