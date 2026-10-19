//! 真实 serve/Core/DbPort + 假 app-server；不使用账户或应用数据。
#[path = "support/t11.rs"]
mod support;
use crewd_server::orchestra::{exec_claude::ClaudeProgram, exec_codex::CodexProgram};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use support::{Client, Fixture, artifact, registry, wait};

fn turn_count(fixture: &Fixture, text: &str) -> usize {
    std::fs::read_to_string(&fixture.log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| {
            call["method"] == "turn/start"
                && call["params"]["input"][0]["text"]
                    .as_str()
                    .is_some_and(|body| body.contains(text))
        })
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_runs_through_serve_and_persists_profile_delivery_and_thread_resume() {
    let fixture = Fixture::new();
    let mut options = fixture.options("ready");
    let orchestra = options.orchestra.as_mut().unwrap();
    orchestra.claude = ClaudeProgram::Disabled;
    orchestra.codex = CodexProgram::Path(artifact("fake_codex"));
    orchestra.codex_env = BTreeMap::from([
        ("HOME".into(), fixture.home.display().to_string()),
        ("SHELL".into(), "/usr/bin/false".into()),
        ("FAKE_CODEX_LOG".into(), fixture.log.display().to_string()),
        ("FAKE_CODEX_MODE".into(), "crew_mcp".into()),
        (
            "FAKE_CODEX_MCP_LOG".into(),
            fixture.root.path().join("mcp.json").display().to_string(),
        ),
    ]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut client = tokio::select! {
        result = &mut server => panic!("serve exited before readiness: {result:?}"),
        client = Client::connect(&fixture.data) => client,
    };
    let crew = client
        .call(
            "crew.create",
            json!({"title":"Synthetic Codex crew","workingDirectory":fixture.work}),
        )
        .await
        .unwrap();
    let crew_id = crew["id"].as_str().unwrap();
    let session = client.call("session.start", json!({"crewId":crew_id,"role":"captain","executor":"codex","brief":"CODEX_BOOTSTRAP_TASK","isolation":false})).await.unwrap();
    let session_id = session["id"].as_str().unwrap();
    wait("initial Codex submission", || {
        turn_count(&fixture, "CODEX_BOOTSTRAP_TASK") == 1
    })
    .await;
    let mcp: Vec<Value> =
        serde_json::from_slice(&std::fs::read(fixture.root.path().join("mcp.json")).unwrap())
            .unwrap();
    assert!(
        mcp.iter()
            .any(|reply| reply["id"] == 3 && reply["result"].get("content").is_some()),
        "actual helper tool response"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let thread = loop {
        let row = client
            .call("session.get", json!({"sessionId":session_id}))
            .await
            .unwrap();
        if row["model"] == "fake-model" {
            assert_eq!(row["effort"], "medium");
            break row["agentSessionId"].as_str().unwrap().to_string();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "confirmed profile not persisted: {row}"
        );
        tokio::task::yield_now().await;
    };
    for text in ["CODEX_FIRST_WAKE", "CODEX_AFTER_RESUME"] {
        if text == "CODEX_AFTER_RESUME" {
            client
                .call(
                    "session.stop",
                    json!({"sessionId":session_id,"reason":"synthetic"}),
                )
                .await
                .unwrap();
            client.stopped(session_id).await;
            // Explicit avatar restoration resumes the original conversation;
            // a message alone must not undo a prior human stop.
            client
                .call("session.resume", json!({"sessionId":session_id}))
                .await
                .unwrap();
        }
        client.call("message.post", json!({"channelId":format!("{crew_id}/lobby"),"text":text,"mentions":[{"kind":"session","targetId":session_id}]})).await.unwrap();
        let until = tokio::time::Instant::now() + Duration::from_secs(5);
        while turn_count(&fixture, text) != 1 {
            if tokio::time::Instant::now() > until {
                let live = client
                    .call("session.inspect", json!({"sessionId":session_id}))
                    .await;
                let wakes = client
                    .call("wake.list", json!({"sessionId":session_id}))
                    .await;
                panic!("missing {text}; live={live:?}; wakes={wakes:?}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let wakes = client
            .call("wake.list", json!({"sessionId":session_id}))
            .await
            .unwrap();
        if wakes["wakes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|wake| !matches!(wake["state"].as_str(), Some("pending" | "inflight")))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "delivery not acknowledged: {wakes}"
        );
        tokio::task::yield_now().await;
    }
    let calls: Vec<Value> = std::fs::read_to_string(&fixture.log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        calls
            .iter()
            .any(|call| call["method"] == "thread/resume" && call["params"]["threadId"] == thread)
    );
    assert_eq!(
        client
            .call("session.get", json!({"sessionId":session_id}))
            .await
            .unwrap()["agentSessionId"],
        thread
    );
    client
        .call(
            "session.stop",
            json!({"sessionId":session_id,"reason":"synthetic"}),
        )
        .await
        .unwrap();
    client.stopped(session_id).await;
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        registry(&fixture.data).is_empty(),
        "all owned processes retired"
    );
}

async fn inspect_request(client: &mut Client, session: &str, method: &str) -> Value {
    let until = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let inspection = client
            .call("session.inspect", json!({"sessionId":session}))
            .await
            .unwrap();
        if inspection["runnerRequest"]["method"] == method {
            return inspection;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "missing request {method}: {inspection}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_answers_native_request_through_real_serve_without_a_key_fallback() {
    let fixture = Fixture::new();
    let mut options = fixture.options("ready");
    let orchestra = options.orchestra.as_mut().unwrap();
    orchestra.claude = ClaudeProgram::Disabled;
    orchestra.codex = CodexProgram::Path(artifact("fake_codex"));
    orchestra.codex_env = BTreeMap::from([
        ("HOME".into(), fixture.home.display().to_string()),
        ("SHELL".into(), "/usr/bin/false".into()),
        ("FAKE_CODEX_LOG".into(), fixture.log.display().to_string()),
        ("FAKE_CODEX_MODE".into(), "human_requests".into()),
    ]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut client = tokio::select! { result=&mut server=>panic!("serve stopped: {result:?}"), client=Client::connect(&fixture.data)=>client };
    let crew = client
        .call(
            "crew.create",
            json!({"title":"Synthetic human requests","workingDirectory":fixture.work}),
        )
        .await
        .unwrap();
    let session=client.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","brief":"CODEX_HUMAN_PATH","isolation":false})).await.unwrap();
    let id = session["id"].as_str().unwrap();
    let inspection =
        inspect_request(&mut client, id, "item/commandExecution/requestApproval").await;
    let fingerprint = inspection["runnerRequest"]["fingerprint"].clone();
    let request_id = inspection["runnerRequest"]["runnerRequestId"]
        .as_str()
        .expect("actual structured request identity is exposed by inspect");
    uuid::Uuid::parse_str(request_id).unwrap();
    assert_ne!(request_id, fingerprint.as_str().unwrap());
    assert_eq!(
        inspection["runnerRequest"]["params"]["command"],
        "git status"
    );
    let response = client
        .call(
            "session.answer",
            json!({"sessionId":id,"fingerprint":fingerprint,"choice":0}),
        )
        .await
        .unwrap();
    assert_eq!(response["cleared"], true);
    let question = inspect_request(&mut client, id, "item/tool/requestUserInput").await;
    let question_id = question["runnerRequest"]["runnerRequestId"]
        .as_str()
        .expect("the next structured request has its own identity");
    uuid::Uuid::parse_str(question_id).unwrap();
    assert_ne!(request_id, question_id);

    assert_eq!(
        question["runnerRequest"]["params"]["questions"][0]["id"],
        "task"
    );
    assert!(
        client
            .call(
                "session.answer",
                json!({"sessionId":id,"fingerprint":fingerprint,"choice":0})
            )
            .await
            .is_err()
    );
    assert!(client.call("session.respondRunnerRequest",json!({"sessionId":id,"fingerprint":question["runnerRequest"]["fingerprint"],"response":{"answers":{"wrong":{"answers":["SYNTHETIC_TASK"]}}}})).await.is_err());
    let response=client.call("session.respondRunnerRequest",json!({"sessionId":id,"fingerprint":question["runnerRequest"]["fingerprint"],"response":{"answers":{"task":{"answers":["SYNTHETIC_TASK"]}}}})).await.unwrap();
    assert_eq!(response["cleared"], true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let live = client
            .call("session.inspect", json!({"sessionId":id}))
            .await
            .unwrap();
        if live["screen"].as_str().is_some_and(|s| s.contains("GOT: ")) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "turn did not continue: {live}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let calls = std::fs::read_to_string(&fixture.log).unwrap();
    let replies: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v.get("method").is_none() && v["id"] == 200)
        .collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["result"]["decision"], "accept");
    client
        .call(
            "session.stop",
            json!({"sessionId":id,"reason":"synthetic test complete"}),
        )
        .await
        .unwrap();
    let _ = stop_tx.send(());
    server.await.unwrap().unwrap();
    assert!(registry(&fixture.data).is_empty());
}

/// A provider-neutral channel must route by session identity, not executor name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_executors_route_mentions_and_reconnect_without_replaying() {
    let fixture = Fixture::new();
    let codex_log = fixture.root.path().join("codex-rpc.log");
    let mut options = fixture.options("ready");
    let orchestra = options.orchestra.as_mut().unwrap();
    orchestra.codex = CodexProgram::Path(artifact("fake_codex"));
    orchestra.codex_env = BTreeMap::from([
        ("HOME".into(), fixture.home.display().to_string()),
        ("SHELL".into(), "/usr/bin/false".into()),
        ("FAKE_CODEX_LOG".into(), codex_log.display().to_string()),
    ]);
    let count_codex = || {
        std::fs::read_to_string(&codex_log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|call| call["method"] == "turn/start")
            .count()
    };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut client = tokio::select! {
        result = &mut server => panic!("serve exited: {result:?}"),
        client = Client::connect(&fixture.data) => client,
    };
    let crew = client
        .call(
            "crew.create",
            json!({"title":"Mixed executor crew","workingDirectory":fixture.work}),
        )
        .await
        .unwrap();
    let cid = crew["id"].as_str().unwrap();
    let claude = client.call("session.start", json!({"crewId":cid,"role":"captain","executor":"claude","brief":"MIXED_CLAUDE_BOOT","isolation":false})).await.unwrap();
    let codex = client.call("session.start", json!({"crewId":cid,"role":"worker","executor":"codex","brief":"MIXED_CODEX_BOOT","isolation":false})).await.unwrap();
    let claude_id = claude["id"].as_str().unwrap();
    let codex_id = codex["id"].as_str().unwrap();
    wait("mixed executor initial turns", || {
        fixture.count("MIXED_CLAUDE_BOOT") == 1 && count_codex() == 1
    })
    .await;
    let channel = format!("{cid}/lobby");
    client.call("message.post", json!({"channelId":channel,"text":"MIXED_TO_CODEX","mentions":[{"kind":"session","targetId":codex_id}]})).await.unwrap();
    wait("Codex recipient", || count_codex() == 2).await;
    assert_eq!(
        fixture.count("MIXED_TO_CODEX"),
        0,
        "unmentioned Claude must not be submitted the Codex wake"
    );
    client.call("message.post", json!({"channelId":channel,"text":"MIXED_TO_CLAUDE","mentions":[{"kind":"session","targetId":claude_id}]})).await.unwrap();
    wait("Claude recipient", || fixture.count("MIXED_TO_CLAUDE") == 1).await;
    for sid in [claude_id, codex_id] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let wakes = client
                .call("wake.list", json!({"sessionId":sid}))
                .await
                .unwrap();
            if wakes["wakes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| !matches!(w["state"].as_str(), Some("pending" | "inflight")))
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "unsettled wake: {wakes}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    drop(client);
    let mut client = Client::connect(&fixture.data).await;
    for _ in 0..20 {
        let messages = client
            .call("message.list", json!({"channelId":channel}))
            .await
            .unwrap();
        for text in ["MIXED_TO_CODEX", "MIXED_TO_CLAUDE"] {
            assert_eq!(
                messages
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|m| m["text"] == text)
                    .count(),
                1
            );
        }
        assert_eq!(
            count_codex(),
            2,
            "read-only reconnect must not submit another Codex turn"
        );
        assert_eq!(
            fixture.count("MIXED_TO_CLAUDE"),
            1,
            "read-only reconnect must not submit another Claude wake"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for sid in [claude_id, codex_id] {
        client
            .call(
                "session.stop",
                json!({"sessionId":sid,"reason":"synthetic mixed executor acceptance"}),
            )
            .await
            .unwrap();
        client.stopped(sid).await;
    }
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(registry(&fixture.data).is_empty());
}
