//! C8: physical generated Agent caller is isolated from server lib tests.
#![cfg(feature = "testkit")]
use crewd_proto::{RpcError, methods};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixStream, unix::OwnedReadHalf};

const LIMIT: Duration = Duration::from_secs(5);

struct Wire {
    reader: BufReader<OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    events: Vec<Value>,
}
impl Wire {
    async fn frame(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let n = tokio::time::timeout(LIMIT, self.reader.read_line(&mut line))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("EOF before functional receipt".into());
        }
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }
    async fn call(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let request = json!({"id":id,"method":method,"params":params});
        tokio::time::timeout(
            LIMIT,
            self.writer.write_all(format!("{request}\n").as_bytes()),
        )
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let deadline = tokio::time::Instant::now() + LIMIT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = tokio::time::timeout(remaining, self.frame())
                .await
                .map_err(|e| e.to_string())??;
            if frame.get("event").is_some() {
                self.events.push(frame);
            } else if frame["id"] == id {
                return Ok(frame);
            } else {
                return Err("unexpected actual response ID".into());
            }
        }
    }
}

async fn s5_complete_file(dir: &std::path::Path, name: &str) -> Result<Value, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match std::fs::read(dir.join(name)) {
            Ok(bytes) => return serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        // Opt-in owned fake failures must remain setup evidence, never a panic oracle.
        match std::fs::File::open(dir.join("fixture.error")) {
            Ok(file) => {
                use std::io::Read;
                let mut bytes = Vec::new();
                file.take(4097)
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("phase {name}: fixture.error read: {e}"))?;
                if bytes.len() > 4096 {
                    return Err(format!(
                        "phase {name}: fixture.error exceeds diagnostic bound"
                    ));
                }
                return Err(format!(
                    "phase {name}: synthetic fixture.error: {}",
                    String::from_utf8_lossy(&bytes)
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("phase {name}: fixture.error open: {e}")),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("phase missing {name}"));
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
fn s5_publish_control(dir: &std::path::Path, name: &str, value: &Value) -> Result<(), String> {
    let temp = dir.join(format!(".{name}.complete-parent"));
    std::fs::write(&temp, serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, dir.join(name)).map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_generated_agent_keeps_attach_watcher_token_after_polled_panic() {
    use crewd_exec::registry::{ProbeResult, ProcessRegistry, probe};
    use crewd_server::{OrchestraServe, ServeOptions};
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let work = root.path().join("work");
    let phase = root.path().join("s5-agent");
    let runtime = root.path().join("runtime");
    let home = root.path().join("home");
    let cache = root.path().join("cache");
    let config = root.path().join("config");
    for dir in [&work, &phase, &home, &cache, &config] {
        std::fs::create_dir(dir).unwrap();
    }
    let debug = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let fake = debug.join("fake_claude");
    let helper = debug.join("crewd");
    assert!(
        fake.is_file() && helper.is_file(),
        "root must fresh-build synthetic binaries before this caller"
    );
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (fault_tx, fault_rx) = tokio::sync::oneshot::channel();
    let opts = ServeOptions {
        orchestra: Some(OrchestraServe {
            claude: crewd_server::orchestra::exec_claude::ClaudeProgram::Path(fake),
            helper: Some(helper),
            runtime_dir: Some(runtime),
            claude_projects_dir: Some(config.join("projects")),
            child_path: Some("/usr/bin:/bin:/usr/sbin:/sbin".into()),
            claude_env: std::collections::BTreeMap::from([
                ("HOME".into(), home.display().to_string()),
                ("SHELL".into(), "/usr/bin/false".into()),
                ("XDG_CACHE_HOME".into(), cache.display().to_string()),
                ("CLAUDE_CONFIG_DIR".into(), config.display().to_string()),
                ("FAKE_CLAUDE_MODE".into(), "s5-agent".into()),
                ("S5_AGENT_FIXTURE_DIR".into(), phase.display().to_string()),
            ]),
            shutdown_limit: Duration::from_secs(10),
            ..Default::default()
        }),
        test_subscriptions_ready: Some(Box::new(move |_| {
            let _ = ready_tx.send(());
        })),
        test_s5_fault_ready: Some(Box::new(move |fault| {
            let _ = fault_tx.send(fault);
        })),
        ..Default::default()
    };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let mut serve = tokio::spawn(crewd_server::serve_with(data.clone(), opts, async {
        let _ = stop_rx.await;
    }));
    struct Catch<F>(std::pin::Pin<Box<F>>);
    impl<F: std::future::Future> std::future::Future for Catch<F> {
        type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.0.as_mut().poll(cx)
            })) {
                Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                Ok(std::task::Poll::Ready(v)) => std::task::Poll::Ready(Ok(v)),
                Err(p) => std::task::Poll::Ready(Err(p)),
            }
        }
    }
    let observed=Catch(Box::pin(async {
        tokio::time::timeout(Duration::from_secs(15),ready_rx).await.map_err(|e|e.to_string())?
            .map_err(|e|e.to_string())?;
        let faults=tokio::time::timeout(Duration::from_secs(15),fault_rx).await.map_err(|e|e.to_string())?
            .map_err(|e|e.to_string())?;
        let stream=UnixStream::connect(data.join("crewd.sock")).await.map_err(|e|e.to_string())?;
        let (reader,writer)=stream.into_split();
        let mut human=Wire{reader:BufReader::new(reader),writer,events:Vec::new()};
        let crew=human.call(801,methods::CREW_CREATE,json!({"title":"S5 owned","workingDirectory":work})).await?;
        let cid=crew["result"]["id"].as_str().ok_or("actual crew creation failed")?.to_owned();
        let started=human.call(802,"session.start",json!({"crewId":cid,"role":"worker","executor":"claude","brief":"S5 owned","isolation":false})).await?;
        let sid=started["result"]["id"].as_str().ok_or("actual session launch failed")?.to_owned();
        let ready=s5_complete_file(&phase,"root.ready.json").await?;
        let pid=ready["pid"].as_i64().and_then(|p|i32::try_from(p).ok()).ok_or("actual root PID receipt")?;
        let deadline=tokio::time::Instant::now()+Duration::from_secs(15);
        let birth=loop {
            let registry=ProcessRegistry::load(&data.join("exec-registry.json")).map_err(|e|e.to_string())?;
            if let Some((_,birth))=registry.entries().into_iter().find(|(id,b)|id==&sid&&b.pid==pid)
                && matches!(probe(pid),ProbeResult::Found(current) if current==birth) {break birth;}

            if tokio::time::Instant::now()>=deadline {return Err("actual driver/kernel enrollment missing".into());}
            tokio::time::sleep(Duration::from_millis(2)).await;
        };
        s5_publish_control(&phase,"spec.json",&json!({"channel":format!("{cid}/lobby")}))?;
        s5_publish_control(&phase,"go",&json!({}))?;
        let attached=s5_complete_file(&phase,"attached.complete.json").await?;
        eprintln!(
            "S5 pre-fault receipts: session_match={} attach_id_match={} attach_success={} crew_match={} worker_role={} tools_id_match={} tools_success={} tools_nonempty={}",
            attached["session"]==sid,
            attached["attach"]["id"]==90101,
            attached["attach"].get("error").is_none(),
            attached["attach"]["result"]["crewId"]==cid,
            attached["attach"]["result"]["role"]=="worker",
            attached["tools"]["id"]==90102,
            attached["tools"].get("error").is_none(),
            attached["tools"]["result"].as_array().is_some_and(|t| !t.is_empty())
        );
        // Ordinary Human801/802 cannot consume this legal Agent90103 fault receipt.
        faults.arm(90103)?;
        s5_publish_control(&phase,"fault.go",&json!({}))?;
        let fault=s5_complete_file(&phase,"fault.complete.json").await?;
        let entered=faults.armed()?.is_none();
        // This functional RED also records EOF/denial without masking it behind a later phase timeout.
        // Positive later oracles are required only after the first exact Internal assertion passes.
        let tail=if fault["fault"]["id"]==90103&&fault["fault"]["error"]["code"]==RpcError::INTERNAL {
        let role=human.call(803,"crew.setCaptain",json!({"crewId":cid,"sessionId":sid})).await?;
        // A denied Human method must not consume the selected Agent fault gate.
        faults.arm(90107)?;
        s5_publish_control(&phase,"role.go",&json!({}))?;
        let complete=s5_complete_file(&phase,"complete.json").await?;
        let socket_closed=s5_complete_file(&phase,"socket.closed.json").await?;
        let denial_gate_preserved=faults.armed()?==Some(90107);
        let rows=human.call(804,methods::MESSAGE_LIST,json!({"channelId":format!("{cid}/lobby")})).await?;
        Some((role,complete,socket_closed,rows,denial_gate_preserved))
        }else{None};
        // Even absent Internal goes through a natural owned-root finish before serve stop.
        s5_publish_control(&phase,"finish",&json!({}))?;
        let deadline=tokio::time::Instant::now()+Duration::from_secs(15);
        let terminal=loop {
            let live=human.call(805,"session.live",json!({"sessionId":sid})).await?;
            if live["result"]["running"]==false {break live;}
            if tokio::time::Instant::now()>=deadline {return Err("actual root driver terminal missing".into());}
            tokio::time::sleep(Duration::from_millis(2)).await;
        };
        drop(human);
        Ok::<_,String>((attached,fault,entered,tail,terminal,birth,cid,sid))
    })).await;
    // Release owned fake even when observations returned Err; root remains driver-owned.
    let _ = s5_publish_control(&phase, "finish", &json!({}));
    let _ = stop_tx.send(());
    let cleanup = tokio::time::timeout(Duration::from_secs(20), &mut serve).await;
    let normal = matches!(&cleanup, Ok(Ok(Ok(()))));
    if cleanup.is_err() {
        serve.abort();
        let _ = serve.await;
    }
    let registry = ProcessRegistry::load(&data.join("exec-registry.json")).map(|r| r.entries());
    let socket_absent = !data.join("crewd.sock").exists();
    eprintln!(
        "S5 actual Agent cleanup normal={normal}, socket_absent={socket_absent}, registry={registry:?}"
    );
    let observed = match observed {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    };
    let (attached, fault, entered, tail, terminal, birth, cid, sid) =
        observed.expect("actual registered Agent completed sameconnection observations");
    assert_eq!(fault["fault"]["id"], 90103);
    assert_eq!(fault["fault"]["error"]["code"], RpcError::INTERNAL);
    assert!(
        fault["fault"]["error"]["message"]
            .as_str()
            .is_some_and(|s| s.contains('#'))
    );
    assert!(
        entered,
        "legal selected Agent fault must be consumed; refusal is not panic proof"
    );
    let (role, complete, socket_closed, rows, denial_gate_preserved) =
        tail.expect("full legal sameconnection positive tail");
    assert_eq!(attached["session"], sid);
    assert_eq!(complete["session"], sid);
    assert_eq!(attached["attach"]["result"]["crewId"], cid);
    assert_eq!(attached["attach"]["result"]["role"], "worker");
    assert!(
        attached["tools"]["result"]
            .as_array()
            .is_some_and(|t| !t.is_empty())
    );
    assert!(role.get("error").is_none());
    assert_eq!(complete["reattach"]["result"]["crewId"], cid);
    assert_eq!(
        complete["reattach"]["result"]["role"], "captain",
        "original generated token still valid"
    );
    assert_ne!(
        complete["before_tools"]["result"],
        complete["after_tools"]["result"]
    );
    assert_eq!(
        complete["after_tools"]["result"],
        complete["repeated_tools"]["result"]
    );
    let events = complete["events"]
        .as_array()
        .expect("actual watcher events");
    assert_eq!(
        events
            .iter()
            .filter(|e| e["event"] == "agent.tools_changed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .find(|e| e["event"] == "agent.tools_changed")
            .unwrap()["data"]["role"],
        "captain"
    );
    assert_eq!(
        complete["denied"]["error"]["code"],
        RpcError::FORBIDDEN,
        "attach never becomes Human"
    );
    assert!(
        denial_gate_preserved,
        "actual kernel refusal precedes selected Agent fault and every effect"
    );
    assert!(!rows.to_string().contains("S5_NO_HUMAN_UPGRADE"));
    assert_eq!(socket_closed["actual_eof"], true);
    assert_eq!(terminal["result"]["running"], false);
    assert!(
        match probe(birth.pid) {
            ProbeResult::Missing => true,
            ProbeResult::Found(now) =>
                now.start_time != birth.start_time || now.boot_id != birth.boot_id,
            ProbeResult::Unreadable(_) => false,
        },
        "actual original root birth disappeared, not intent"
    );
    assert!(
        normal && socket_absent && registry.is_ok_and(|r| r.is_empty()),
        "normal serve/storage/socket/registry cleanup required"
    );
}
