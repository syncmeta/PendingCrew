//! Actual socket + production executor enrollment + owned synthetic root/grandchild.
mod support {
    pub mod t11;
}
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use support::t11::*;
struct Catch<F: Future>(Pin<Box<F>>);
impl<F: Future> Future for Catch<F> {
    type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.get_mut().0.as_mut().poll(cx)
        })) {
            Ok(Poll::Ready(v)) => Poll::Ready(Ok(v)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(p) => Poll::Ready(Err(p)),
        }
    }
}
// Failure diagnostics preserve the original observation panic; successful await is
// not confused with a successful JoinHandle or successful serve result.
async fn report_s1_cleanup_failure(
    cleanup: Result<
        Result<anyhow::Result<()>, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
    server: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    match cleanup {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            eprintln!("S1 cleanup serve failure after original observation panic: {error:#}")
        }
        Ok(Err(error)) => {
            eprintln!("S1 cleanup join failure after original observation panic: {error}")
        }
        Err(error) => {
            eprintln!("S1 cleanup timeout after original observation panic: {error}");
            server.abort();
            // Owned-task cancellation is fallback, never reported as normal shutdown.
            match server.await {
                Ok(Ok(())) => eprintln!("S1 timeout fallback joined completed serve"),
                Ok(Err(error)) => eprintln!("S1 timeout fallback serve failure: {error:#}"),
                Err(error) => eprintln!("S1 timeout fallback join result: {error}"),
            }
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_actual_managed_root_and_grandchild_cannot_mix_human_routes() {
    let f = Fixture::new();
    let fixture = f.root.path().join("s1-peer");
    std::fs::create_dir(&fixture).unwrap();
    let mut opts = f.options("s1-peer");
    opts.orchestra
        .as_mut()
        .unwrap()
        .claude_env
        .insert("S1_PEER_FIXTURE_DIR".into(), fixture.display().to_string());
    let (subscriptions_tx, subscriptions_rx) = tokio::sync::oneshot::channel();
    opts.test_subscriptions_ready = Some(Box::new(move |counter| {
        let _ = subscriptions_tx.send(counter);
    }));
    let (stop, rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
        let _ = rx.await;
    }));
    let observed=Catch(Box::pin(async {
        let subscriptions = tokio::time::timeout(Duration::from_secs(15), subscriptions_rx).await
            .expect("actual bound listener subscription witness").expect("read-only counter receipt");
        let mut human=Client::connect(&f.data).await;
        let crew=human.call("crew.create",json!({"title":"S1 disposable","workingDirectory":f.work})).await.unwrap();
        let cid=crew["id"].as_str().unwrap().to_string();let channel=format!("{cid}/lobby");
        // Genuine parent Human positive, no declared auth token.
        let positive=human.call("message.post",json!({"channelId":channel,"text":"S1_LEGITIMATE_HUMAN","mentions":[]})).await;
        let subscribed = human.call("events.subscribe", json!({"topics":["crew"]})).await;
        let subscriptions_before = subscriptions.count();
        let started=human.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"S1 disposable","isolation":false})).await.unwrap();
        let sid=started["id"].as_str().unwrap().to_string();
        wait("owned program readiness",||fixture.join("root.ready").is_file()).await;
        let pid=std::fs::read_to_string(fixture.join("root.ready")).unwrap().parse::<i32>().unwrap();
        // Registry proof comes from actual driver record, not helper's claimed PID alone.
        wait("actual driver birth enrollment",||registry(&f.data).iter().any(|(s,r)|s==&sid&&r.pid==pid)).await;
        std::fs::write(fixture.join("requests.json"),serde_json::to_vec(&json!({"socket":f.data.join("crewd.sock"),"channel":channel})).unwrap()).unwrap();
        std::fs::write(fixture.join("go"),b"").unwrap();
        wait("actual child and root complete responses",||fixture.join("child.responses.json").is_file()&&fixture.join("root.responses.json").is_file()&&fixture.join("child.exit.json").is_file()).await;
        let root:Value=serde_json::from_slice(&std::fs::read(fixture.join("root.responses.json")).unwrap()).unwrap();
        let child:Value=serde_json::from_slice(&std::fs::read(fixture.join("child.responses.json")).unwrap()).unwrap();
        let helper:Value=serde_json::from_slice(&std::fs::read(fixture.join("helper.positive.json")).unwrap()).unwrap();
        let exit:Value=serde_json::from_slice(&std::fs::read(fixture.join("child.exit.json")).unwrap()).unwrap();
        let rows=human.call("message.list",json!({"channelId":channel})).await.unwrap();
        std::fs::write(fixture.join("finish"),b"").unwrap();
        let subscriptions_after = subscriptions.count();
        Ok::<_,anyhow::Error>((positive,root,child,exit,rows,helper,subscribed,subscriptions_before,subscriptions_after))
    })).await;
    // Always release the disposable synthetic root then stop intake/actor/storage and await real serve join.
    let _ = std::fs::write(fixture.join("finish"), b"");
    let _ = stop.send(());
    let cleanup = tokio::time::timeout(Duration::from_secs(20), &mut server).await;
    match observed {
        Err(p) => {
            report_s1_cleanup_failure(cleanup, &mut server).await;
            std::panic::resume_unwind(p)
        }
        Ok(v) => {
            cleanup
                .expect("bounded normal serve cleanup")
                .expect("serve task")
                .expect("serve close");
            let (
                positive,
                root,
                child,
                exit,
                rows,
                helper,
                subscribed,
                subscriptions_before,
                subscriptions_after,
            ) = v.expect("observations before normal close");
            assert!(positive.is_ok(), "real Human positive");
            assert!(
                subscribed.is_ok(),
                "real Human ordinary subscription positive"
            );
            assert_eq!(
                subscriptions_before, 1,
                "actual client receiver creation positive"
            );
            assert_eq!(
                subscriptions_after, subscriptions_before,
                "denied root/grandchild ordinary subscriptions must create no client Hub receiver, even if later dropped"
            );
            assert_eq!(
                helper["normal"], true,
                "generated actual crewd helper closes normally"
            );
            assert_eq!(helper["code"], 0);
            assert!(helper["responses"][0].get("result").is_some());
            assert!(
                helper["responses"][1]["result"]["tools"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty()),
                "actual generated MCP helper positive"
            );
            assert_eq!(exit["normal"], true);
            assert_eq!(exit["code"], 0);
            for responses in [&root, &child] {
                for index in [1, 2] {
                    assert_eq!(
                        responses[index]["error"]["code"],
                        crewd_proto::RpcError::FORBIDDEN,
                        "managed peer human route denied before dispatch"
                    );
                }
            }
            assert!(
                root[3].get("result").is_some(),
                "legal actual token attach positive"
            );
            assert!(
                root[4].get("result").is_some(),
                "legal helper tools positive"
            );
            assert_eq!(
                root[5]["error"]["code"],
                crewd_proto::RpcError::FORBIDDEN,
                "attach never upgrades Human"
            );
            assert!(
                !rows.to_string().contains("S1_SYNTHETIC_")
                    && !rows.to_string().contains("S1_AFTER_ATTACH"),
                "denied routes commit no Human message"
            );
            assert!(
                registry(&f.data).is_empty(),
                "normal actual driver reap clears active orphan registry"
            );
        }
    }
}

fn original_birth_gone(id: &crewd_exec::registry::ProcessIdentity) -> bool {
    match crewd_exec::registry::probe(id.pid) {
        crewd_exec::registry::ProbeResult::Missing => true,
        crewd_exec::registry::ProbeResult::Found(now) => {
            now.start_time != id.start_time || now.boot_id != id.boot_id
        }
        crewd_exec::registry::ProbeResult::Unreadable(_) => false,
    }
}
// Error-path containment only. Non-child exit is proved by typed kernel identity,
// never fabricated wait code; old observation panic is always rethrown afterward.
async fn cleanup_recorded_orphan_after_failure(dir: &std::path::Path) {
    use crewd_exec::registry::{ProbeResult, ProcessIdentity, probe};
    let recorded = std::fs::read(dir.join("orphan.spawn.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ProcessIdentity>(&bytes).ok());
    let Some(original) = recorded else {
        eprintln!("S1 orphan error cleanup: no valid recorded birth; no PID signal authorized");
        return;
    };
    if original.pid <= 0 {
        eprintln!("S1 orphan error cleanup: invalid recorded PID; no signal");
        return;
    }
    let same_birth = |now: &ProcessIdentity| {
        now.pid == original.pid
            && now.start_time == original.start_time
            && now.boot_id == original.boot_id
    };
    // Finish was already published by the caller before normal serve join.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match probe(original.pid) {
            ProbeResult::Missing => {
                eprintln!("S1 recorded orphan birth gone after finish; non-child wait unavailable");
                return;
            }
            ProbeResult::Found(now) if !same_birth(&now) => {
                eprintln!("S1 recorded orphan birth gone; replacement PID untouched");
                return;
            }
            ProbeResult::Unreadable(error) => {
                eprintln!("S1 orphan identity unreadable: {error:?}; no signal");
                return;
            }
            ProbeResult::Found(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // Reprobe immediately before signaling. Never signal a different/unreadable birth/group.
    match probe(original.pid) {
        ProbeResult::Found(now) if same_birth(&now) => {
            // SAFETY: positive PID is this owned, typed and freshly rechecked fixture birth.
            let result = unsafe { libc::kill(original.pid, libc::SIGKILL) };
            let error = if result == -1 {
                Some(std::io::Error::last_os_error())
            } else {
                None
            };
            eprintln!(
                "S1 orphan exact-birth fallback SIGKILL result={result}, error={error:?}; not normal cleanup"
            );
        }
        other => {
            eprintln!("S1 orphan fallback identity changed/unreadable: {other:?}; no signal");
            return;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match probe(original.pid) {
            ProbeResult::Missing => {
                eprintln!("S1 killed owned orphan birth disappeared; non-child wait unavailable");
                return;
            }
            ProbeResult::Found(now) if !same_birth(&now) => {
                eprintln!("S1 killed original birth gone; replacement untouched");
                return;
            }
            ProbeResult::Unreadable(error) => {
                eprintln!("S1 post-kill identity unreadable: {error:?}; cleanup unproven");
                return;
            }
            ProbeResult::Found(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            eprintln!("S1 orphan exact-birth cleanup deadline; surviving birth, cleanup UNPROVEN");
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
async fn reparent_case(scenario: &str) {
    let f = Fixture::new();
    let dir = f.root.path().join("s1-reparent");
    std::fs::create_dir(&dir).unwrap();
    let mut opts = f.options("s1-peer");
    opts.orchestra
        .as_mut()
        .unwrap()
        .claude_env
        .insert("S1_PEER_FIXTURE_DIR".into(), dir.display().to_string());
    let (stop, rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(f.data.clone(), opts, async {
        let _ = rx.await;
    }));
    let observed=Catch(Box::pin(async {
        let mut human=Client::connect(&f.data).await;
        let crew=human.call("crew.create",json!({"title":"S1 reparent disposable","workingDirectory":f.work})).await.unwrap();
        let cid=crew["id"].as_str().unwrap().to_owned();let channel=format!("{cid}/lobby");
        let started=human.call("session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"S1 reparent","isolation":false})).await.unwrap();
        let sid=started["id"].as_str().unwrap().to_owned();
        wait("real original driver root ready",||dir.join("root.ready").is_file()).await;
        let rootpid=std::fs::read_to_string(dir.join("root.ready")).unwrap().parse::<i32>().unwrap();
        wait("real original driver enrolled",||registry(&f.data).iter().any(|(s,p)|s==&sid&&p.pid==rootpid)).await;
        let birth=registry(&f.data).into_iter().find(|(s,p)|s==&sid&&p.pid==rootpid).unwrap().1;
        std::fs::write(dir.join("actual.root.birth.json"),serde_json::to_vec(&birth).unwrap()).unwrap();
        std::fs::write(dir.join("requests.json"),serde_json::to_vec(&json!({"socket":f.data.join("crewd.sock"),"channel":channel,"scenario":scenario})).unwrap()).unwrap();
        std::fs::write(dir.join("go"),b"").unwrap();
        wait("orphan before and after actual parent change",||dir.join("orphan.after.json").is_file()&&dir.join("intermediate.exit.json").is_file()&&dir.join("orphan.terminal.intent.json").is_file()).await;
        let child:crewd_exec::registry::ProcessIdentity=serde_json::from_slice(&std::fs::read(dir.join("orphan.spawn.json")).unwrap()).unwrap();
        wait("actual former child birth disappeared",||original_birth_gone(&child)).await;
        if scenario=="root-exit" { human.stopped(&sid).await; }
        let before:Value=serde_json::from_slice(&std::fs::read(dir.join("orphan.before.json")).unwrap()).unwrap();
        let after:Value=serde_json::from_slice(&std::fs::read(dir.join("orphan.after.json")).unwrap()).unwrap();
        let exit:Value=serde_json::from_slice(&std::fs::read(dir.join("intermediate.exit.json")).unwrap()).unwrap();
        let rows=human.call("message.list",json!({"channelId":channel})).await.unwrap();
        // Genuine independent Human connection and the original Human connection both
        // remain usable after restricted clients mixed hello/post/subscribe routes.
        let old_positive=human.call("crew.list",json!({})).await;
        let mut fresh_human=Client::connect(&f.data).await;
        let fresh_positive=fresh_human.call("crew.create",json!({"title":"S1 independent Human","workingDirectory":f.work})).await;
        Ok::<_,anyhow::Error>((before,after,exit,rows,old_positive,fresh_positive,scenario=="root-exit" && original_birth_gone(&birth)))
    })).await;
    let _ = std::fs::write(dir.join("finish"), b"");
    let _ = stop.send(());
    let closed = tokio::time::timeout(Duration::from_secs(20), &mut server).await;
    match observed {
        Err(p) => {
            cleanup_recorded_orphan_after_failure(&dir).await;
            // Only this owned synthetic directory, after stop/join, before TempDir drop.
            // Keep original panic; diagnostics never replace a functional assertion.
            for name in ["orphan.phases.jsonl", "orphan.stderr.log"] {
                use std::io::Read;
                let mut bytes = Vec::new();
                match std::fs::File::open(dir.join(name)) {
                    Ok(file) => {
                        let read = file.take(16 * 1024).read_to_end(&mut bytes);
                        eprintln!(
                            "S1 owned orphan {name} (bounded16KiB, read={read:?}): {}",
                            String::from_utf8_lossy(&bytes)
                        );
                    }
                    Err(error) => eprintln!("S1 owned orphan {name} unavailable: {error}"),
                }
            }
            report_s1_cleanup_failure(closed, &mut server).await;
            std::panic::resume_unwind(p)
        }
        Ok(v) => {
            closed
                .expect("normal bounded serve closure")
                .expect("serve join")
                .expect("serve result");
            let (before, after, exit, rows, old_positive, fresh_positive, root_gone) =
                v.expect("real observations");
            assert_eq!(exit["normal"], true, "owned intermediate normally waited");
            assert_eq!(exit["code"], 0);
            assert_ne!(
                after["parent_before"], after["parent_after"],
                "kernel actual reparent barrier"
            );
            let before_birth: crewd_exec::registry::ProcessIdentity =
                serde_json::from_value(after["birth_before"].clone())
                    .expect("actual typed before birth");
            let after_birth: crewd_exec::registry::ProcessIdentity =
                serde_json::from_value(after["birth_after"].clone())
                    .expect("actual typed after birth");
            assert_eq!(before_birth.pid, after_birth.pid);
            assert_eq!(
                before_birth.start_time, after_birth.start_time,
                "same kernel caller birth, reused socket"
            );
            assert_eq!(before_birth.boot_id, after_birth.boot_id);
            assert!(
                old_positive.is_ok() && fresh_positive.is_ok(),
                "normal Human clients remain usable after three restricted entry classes"
            );
            if scenario == "root-exit" {
                assert!(
                    root_gone,
                    "actual original enrolled birth gone before reused/new peer requests"
                );
            }
            for responses in [&before, &after["responses"], &after["reconnect"]] {
                for i in [1, 2] {
                    assert_eq!(
                        responses[i]["error"]["code"],
                        crewd_proto::RpcError::FORBIDDEN,
                        "same/reconnected orphan cannot become Human after parent loss"
                    );
                }
            }
            assert!(
                !rows.to_string().contains("S1_ORPHAN_"),
                "restricted requests leave no committed Human post"
            );
            assert!(
                registry(&f.data).is_empty(),
                "normal original driver shutdown removes active registry"
            );
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_actual_doublefork_reparent_reuses_connection_without_human_elevation() {
    reparent_case("reparent").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_actual_registered_root_exit_never_elevates_retained_or_reconnected_descendant() {
    reparent_case("root-exit").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_actual_socket_mode_and_same_process_human_without_orchestra_are_compatible() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let (stop, rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(
        f.data.clone(),
        Default::default(),
        async {
            let _ = rx.await;
        },
    ));
    let observed = Catch(Box::pin(async {
        let mut human = Client::connect(&f.data).await;
        let mode = std::fs::metadata(f.data.join("crewd.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let hello = human
            .call(
                "system.hello",
                json!({"client":{"name":"synthetic-human","version":"0"}}),
            )
            .await;
        let subscribed = human
            .call("events.subscribe", json!({"topics":["crew"]}))
            .await;
        let created = human
            .call(
                "crew.create",
                json!({"title":"S1 Human three-entry positive","workingDirectory":f.work}),
            )
            .await;
        (mode, hello, subscribed, created)
    }))
    .await;
    let _ = stop.send(());
    let closed = tokio::time::timeout(Duration::from_secs(20), &mut server).await;
    match observed {
        Err(p) => {
            report_s1_cleanup_failure(closed, &mut server).await;
            std::panic::resume_unwind(p)
        }
        Ok((mode, hello, subscribed, created)) => {
            closed
                .expect("normal close")
                .expect("serve join")
                .expect("serve result");
            assert_eq!(mode, 0o600, "actual socket owner-only mode");
            assert!(
                hello.is_ok() && subscribed.is_ok() && created.is_ok(),
                "real normal Human connection can use declared/subscribe/ordinary APIs"
            );
        }
    }
}
#[tokio::test]
async fn s1_socket_invalid_byte_path_returns_before_storage_or_actor_initialization() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let too_long = root.path().join("界".repeat(70));
    let nul = root
        .path()
        .join(std::ffi::OsString::from_vec(b"data\0invalid".to_vec()));
    let mut observations = Vec::new();
    for path in [&too_long, &nul] {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            crewd_server::serve_with(path.clone(), Default::default(), async {}),
        )
        .await;
        let exists = path.exists();
        observations.push((result, exists));
    }
    // Each call has returned; no owned serve/actor subprocess was started by this fixture.
    for (result, exists) in observations {
        assert!(
            result.expect("invalid path must return bounded").is_err(),
            "invalid socket bytes fail"
        );
        assert!(
            !exists,
            "socket byte validation precedes any data/store initialization"
        );
    }
}
// OWN append candidate after accepted S1 fixture/main flag stages. NOTCOMPILED/NOTRUN.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_actual_sibling_human_ndjson_client_is_qualified_by_real_launcher_birth() {
    use crewd_exec::registry::{self, ParentProbe};
    use std::process::Stdio;
    use tokio::process::Command;
    let f = Fixture::new();
    let dir = f.root.path().join("s1-sibling");
    std::fs::create_dir(&dir).unwrap();
    let mut daemon = Command::new(artifact("crewd"));
    daemon
        .env_clear()
        .env("PATH", CHILD_PATH)
        .env("HOME", &f.home)
        .env("XDG_CACHE_HOME", &f.cache)
        .env("CLAUDE_CONFIG_DIR", &f.config)
        .env("TMPDIR", f.root.path())
        .env("SHELL", "/usr/bin/false")
        .env("CREWD_DATA_DIR", &f.data)
        .env("CREWD_CLAUDE", artifact("fake_claude"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["serve", "--claude"])
        .arg(artifact("fake_claude"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(dir.join("daemon.stderr")).unwrap())
        .kill_on_drop(true);
    let mut daemon = daemon.spawn().unwrap();
    let daemon_pid = daemon.id().unwrap();
    let observed = Catch(Box::pin(async {
        let mut parent_client = Client::connect(&f.data).await;
        let crew = parent_client
            .call(
                "crew.create",
                json!({"title":"S1 sibling owned", "workingDirectory":f.work}),
            )
            .await
            .unwrap();
        let channel = format!("{}/lobby", crew["id"].as_str().unwrap());
        let ParentProbe::Found {
            identity: daemon_birth,
            parent: daemon_parent,
        } = registry::parent(daemon_pid as i32)
        else {
            panic!("real daemon birth required");
        };
        let mut helper = Command::new(artifact("fake_claude"));
        helper
            .env_clear()
            .env("PATH", CHILD_PATH)
            .env("HOME", &f.home)
            .env("XDG_CACHE_HOME", &f.cache)
            .env("CLAUDE_CONFIG_DIR", &f.config)
            .env("TMPDIR", f.root.path())
            .env("SHELL", "/usr/bin/false")
            .arg("--s1-peer-child")
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.join("helper.stderr")).unwrap())
            .kill_on_drop(true);
        let mut helper = helper.spawn().unwrap();
        let helper_pid = helper.id().unwrap();
        wait("actual sibling helper ready before commands", || {
            dir.join("child.ready").exists()
        })
        .await;
        let ready_pid = std::fs::read_to_string(dir.join("child.ready"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let ParentProbe::Found {
            identity: helper_birth,
            parent: helper_parent,
        } = registry::parent(helper_pid as i32)
        else {
            panic!("real sibling birth required");
        };
        let ParentProbe::Found {
            identity: launcher_birth,
            ..
        } = registry::parent(std::process::id() as i32)
        else {
            panic!("real launcher birth required");
        };
        // Complete command content precedes the contentless go marker.
        std::fs::write(
            dir.join("requests.json"),
            serde_json::to_vec(&json!({"socket":f.data.join("crewd.sock"), "channel":channel}))
                .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("go"), []).unwrap();
        let helper_exit = tokio::time::timeout(Duration::from_secs(15), helper.wait())
            .await
            .expect("normal sibling helper close")
            .unwrap();
        let responses: Value =
            serde_json::from_slice(&std::fs::read(dir.join("child.responses.json")).unwrap())
                .unwrap();
        let rows = parent_client
            .call("message.list", json!({"channelId":channel}))
            .await
            .unwrap();
        (
            daemon_birth,
            daemon_parent,
            helper_birth,
            helper_parent,
            launcher_birth,
            ready_pid,
            helper_pid,
            helper_exit,
            responses,
            rows,
        )
    }))
    .await;
    // Exact owned daemon PID only; no process-name kill or authority override.
    let signal = Command::new("/bin/kill")
        .args(["-TERM", &daemon_pid.to_string()])
        .status()
        .await;
    let closed = tokio::time::timeout(Duration::from_secs(20), daemon.wait()).await;
    let data_cleanup = !f.data.join("crewd.sock").exists() && registry(&f.data).is_empty();
    let values = match observed {
        Ok(v) => v,
        Err(original) => {
            eprintln!(
                "S1 sibling cleanup after original failure: signal={signal:?} close={closed:?} files_closed={data_cleanup}"
            );
            std::panic::resume_unwind(original)
        }
    };
    assert!(signal.unwrap().success());
    assert!(
        closed
            .expect("bounded normal owned daemon exit")
            .unwrap()
            .success()
    );
    assert!(data_cleanup);
    let (
        daemon_birth,
        daemon_parent,
        helper_birth,
        helper_parent,
        launcher_birth,
        ready_pid,
        helper_pid,
        helper_exit,
        responses,
        rows,
    ) = values;
    assert_eq!(
        (daemon_parent, helper_parent),
        (launcher_birth.pid, launcher_birth.pid)
    );
    assert_ne!(daemon_birth.pid, helper_birth.pid);
    assert_eq!(ready_pid, helper_pid);
    assert_eq!(helper_birth.pid, helper_pid as i32);
    assert!(helper_exit.success() && helper_exit.code() == Some(0));
    for (index, id) in [(0, 1), (1, 2), (2, 3)] {
        assert_eq!(responses[index]["id"], id);
        assert!(
            responses[index].get("result").is_some(),
            "actual sibling hello/post/subscribe must succeed"
        );
    }
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "S1_SYNTHETIC_child" && m["senderKind"] == "human")
    );
}
