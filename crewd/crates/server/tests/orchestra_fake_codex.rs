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

/// Original serve/Core/real Unix RPC seam. Each absent capability is observed
/// only after normal fixture cleanup, so an initial RED leaves no owned child.
async fn codex_control_case(operation: &str) -> (Result<Value, Value>, Vec<Value>) {
    let fixture = Fixture::new();
    let mut options = fixture.options("ready");
    let orchestra = options.orchestra.as_mut().unwrap();
    orchestra.claude = ClaudeProgram::Disabled;
    orchestra.codex = CodexProgram::Path(artifact("fake_codex"));
    orchestra.codex_env =
        BTreeMap::from([("FAKE_CODEX_LOG".into(), fixture.log.display().to_string())]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut client = Client::connect(&fixture.data).await;
    let crew = client
        .call(
            "crew.create",
            json!({"title":"Controls synthetic","workingDirectory":fixture.work}),
        )
        .await
        .unwrap();
    let session=client.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","brief":"Implement the requested change.\n你是 is user text and stays intact.","title":"Human title wins","isolation":false})).await.unwrap();
    let sid = session["id"].as_str().unwrap();
    wait("first task accepted", || {
        turn_count(&fixture, "Implement the requested change.") == 1
    })
    .await;
    // A turn/start log precedes its ACK/completion. Wait for the existing durable usage
    // event emitted after fake completion before capturing a command CAS target.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let usage = client
            .call("session.usage", json!({"sessionId":sid}))
            .await
            .unwrap();
        if !usage["items"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }

    let target = client
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let result = if operation == "initial-title" {
        client
            .call("session.codex.inspect", json!({"sessionId":sid}))
            .await
    } else if operation == "history" {
        client
            .call(
                "session.codex.history",
                json!({"sessionId":sid,"limit":10,"itemsView":"full"}),
            )
            .await
    } else {
        let hello = client
            .call(
                "system.hello",
                json!({"clientName":"controls-fixture","clientVersion":"1"}),
            )
            .await
            .unwrap();
        let params = match operation {
            "session.codex.configure" => {
                json!({"target":target,"patch":{"model":"changed-model","effort":"high","fast":true}})
            }
            _ => unreachable!(),
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut result=client.call("command.submit",json!({"requestId":request_id,"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":operation,"params":params}})).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while result.as_ref().is_ok_and(|r| r["phase"] == "dispatched")
            && tokio::time::Instant::now() < deadline
        {
            result = client
                .call(
                    "command.lookup",
                    json!({"requestId":request_id,"dbId":hello["runtimeIdentity"]["dbId"]}),
                )
                .await;
            tokio::task::yield_now().await;
        }
        result
    };
    client
        .call(
            "session.stop",
            json!({"sessionId":sid,"reason":"synthetic cleanup"}),
        )
        .await
        .unwrap();
    client.stopped(sid).await;
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(registry(&fixture.data).is_empty());
    let calls = std::fs::read_to_string(&fixture.log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (result, calls)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controls_public_settings_reach_provider_with_receipt() {
    let (result, calls) = codex_control_case("session.codex.configure").await;
    let receipt = result.expect("the typed public configure command must be callable");
    assert_eq!(receipt["phase"], "confirmed", "{receipt}");
    assert!(calls.iter().any(
        |c| c["method"] == "thread/settings/update" && c["params"]["model"] == "changed-model"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controls_native_title_is_independent_of_original_task_body() {
    let (state, calls) = codex_control_case("initial-title").await;
    let state = state.expect("actual native title state");
    assert_eq!(state["title"]["status"], "synced", "{state}");
    assert_eq!(state["title"]["native"], "Human title wins");
    assert!(
        calls
            .iter()
            .any(|c| c["method"] == "thread/name/set" && c["params"]["name"] == "Human title wins"),
        "desired PCW title alone is not native naming"
    );
    let turns: Vec<_> = calls
        .iter()
        .filter(|c| c["method"] == "turn/start")
        .collect();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0]["params"]["input"][0]["text"],
        "Implement the requested change.\n你是 is user text and stays intact."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controls_history_is_read_only_provider_page() {
    let (result, calls) = codex_control_case("history").await;
    let page = result.expect("the scoped native history read must be callable");
    assert!(page["items"].is_array());
    assert!(calls.iter().any(|c| c["method"] == "thread/turns/list"));
    assert_eq!(
        calls.iter().filter(|c| c["method"] == "turn/start").count(),
        1,
        "history must not start another turn"
    );
}

async fn control_receipt(
    client: &mut Client,
    identity: &Value,
    id: &str,
    operation: &str,
    params: Value,
) -> Value {
    let mut result=client.call("command.submit",json!({"requestId":id,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":operation,"params":params}})).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while result["phase"] == "dispatched" && tokio::time::Instant::now() < deadline {
        result = client
            .call(
                "command.lookup",
                json!({"requestId":id,"dbId":identity["dbId"]}),
            )
            .await
            .unwrap();
        tokio::task::yield_now().await;
    }
    result
}
fn native_calls(fixture: &Fixture, method: &str) -> Vec<Value> {
    std::fs::read_to_string(&fixture.log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["method"] == method)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controls_full_public_lifecycle_preserves_identity_receipts_and_read_only_metadata() {
    let fixture = Fixture::new();
    let mut options = fixture.options("ready");
    let o = options.orchestra.as_mut().unwrap();
    o.claude = ClaudeProgram::Disabled;
    o.codex = CodexProgram::Path(artifact("fake_codex"));
    o.codex_env = BTreeMap::from([
        ("FAKE_CODEX_LOG".into(), fixture.log.display().to_string()),
        ("FAKE_CODEX_MODE".into(), "slow_turn".into()),
    ]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut c = Client::connect(&fixture.data).await;
    let hello = c.call("system.hello", json!({})).await.unwrap();
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("session.codex"))
    );
    let identity = hello["runtimeIdentity"].clone();
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Codex controls","workingDirectory":fixture.work}),
        )
        .await
        .unwrap();
    let row=c.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","brief":"","title":"Control source","isolation":false})).await.unwrap();
    let sid = row["id"].as_str().unwrap();
    wait("source native name", || {
        native_calls(&fixture, "thread/name/set").len() == 1
    })
    .await;
    let original = c
        .call("session.get", json!({"sessionId":sid}))
        .await
        .unwrap()["agentSessionId"]
        .clone();
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let params = json!({"target":target,"patch":{"model":"changed-model","fast":true,"approvalPolicy":"onRequest","approvalsReviewer":"user","sandbox":{"type":"readOnly","networkAccess":false},"collaborationMode":"plan","summary":"concise"}});
    let receipt = control_receipt(
        &mut c,
        &identity,
        &id,
        "session.codex.configure",
        params.clone(),
    )
    .await;
    assert_eq!(receipt["phase"], "confirmed", "{receipt}");
    assert_eq!(receipt["result"]["application"], "followingTurns");
    let repeated = control_receipt(
        &mut c,
        &identity,
        &id,
        "session.codex.configure",
        params.clone(),
    )
    .await;
    assert_eq!(receipt, repeated);
    assert_eq!(native_calls(&fixture, "thread/settings/update").len(), 1);
    let mut changed = params.clone();
    changed["patch"]["model"] = json!("another-model");
    assert!(c.call("command.submit",json!({"requestId":id,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.codex.configure","params":changed}})).await.is_err());
    let inspect = c
        .call("session.codex.inspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(inspect["settingsStatus"], "accepted");
    assert_eq!(inspect["desiredSettings"]["collaborationMode"], "plan");
    assert!(
        inspect["settings"]["collaborationMode"].is_null(),
        "absent effective mode must not be invented from desired settings"
    );
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let queue_id = uuid::Uuid::new_v4().to_string();
    let queued = control_receipt(
        &mut c,
        &identity,
        &queue_id,
        "session.codex.queue",
        json!({"target":target,"text":"Exact queued original task"}),
    )
    .await;
    assert_eq!(queued["phase"], "confirmed");
    assert_eq!(queued["result"]["application"], "queued");
    assert_eq!(queued["result"]["providerAccepted"], false);
    wait("one queued turn", || {
        native_calls(&fixture, "turn/start").len() == 1
    })
    .await;
    assert_eq!(
        native_calls(&fixture, "turn/start")[0]["params"]["clientUserMessageId"],
        queue_id
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let target = loop {
        let t = c
            .call("session.controlTarget", json!({"sessionId":sid}))
            .await
            .unwrap();
        if t["turnId"].is_string() {
            break t;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    };
    let steered = control_receipt(
        &mut c,
        &identity,
        &uuid::Uuid::new_v4().to_string(),
        "session.codex.steer",
        json!({"target":target,"text":"Exact same-turn correction"}),
    )
    .await;
    assert_eq!(steered["phase"], "confirmed", "{steered}");
    assert_eq!(steered["result"]["application"], "currentTurn");
    assert_eq!(native_calls(&fixture, "turn/start").len(), 1);
    assert_eq!(native_calls(&fixture, "turn/steer").len(), 1);
    let stale = control_receipt(
        &mut c,
        &identity,
        &uuid::Uuid::new_v4().to_string(),
        "session.codex.steer",
        json!({"target":target,"text":"Never deliver stale input"}),
    )
    .await;
    assert_eq!(stale["phase"], "suppressed");
    assert_eq!(native_calls(&fixture, "turn/steer").len(), 1);
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let interrupted = control_receipt(
        &mut c,
        &identity,
        &uuid::Uuid::new_v4().to_string(),
        "session.interrupt",
        target,
    )
    .await;
    assert_eq!(interrupted["phase"], "confirmed", "{interrupted}");
    assert_eq!(interrupted["result"]["application"], "started");
    for operation in ["session.codex.compact", "session.codex.review"] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let live = c
                .call("session.live", json!({"sessionId":sid}))
                .await
                .unwrap();
            if live["working"] == false {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        let target = c
            .call("session.controlTarget", json!({"sessionId":sid}))
            .await
            .unwrap();
        let p = if operation.ends_with("review") {
            json!({"target":target,"reviewTarget":{"type":"uncommittedChanges"}})
        } else {
            json!({"target":target})
        };
        let receipt = control_receipt(
            &mut c,
            &identity,
            &uuid::Uuid::new_v4().to_string(),
            operation,
            p,
        )
        .await;
        assert_eq!(receipt["phase"], "confirmed", "{receipt}");
        assert_eq!(receipt["result"]["application"], "started");
    }
    c.call(
        "session.stop",
        json!({"sessionId":sid,"reason":"metadata-only phase"}),
    )
    .await
    .unwrap();
    c.stopped(sid).await;
    let turns = native_calls(&fixture, "turn/start").len();
    let starts = native_calls(&fixture, "thread/start").len();
    let resumes = native_calls(&fixture, "thread/resume").len();
    for method in [
        "session.codex.history",
        "session.codex.models",
        "session.codex.tools",
    ] {
        let value = c.call(method, json!({"sessionId":sid})).await.unwrap();
        assert!(value.is_object(), "{method}: {value}");
    }
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let named = control_receipt(
        &mut c,
        &identity,
        &uuid::Uuid::new_v4().to_string(),
        "session.codex.titleSet",
        json!({"target":target,"title":"Renamed source"}),
    )
    .await;
    assert_eq!(named["phase"], "confirmed", "{named}");
    let state = c
        .call("session.codex.inspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(state["running"], false);
    assert_eq!(state["title"]["native"], "Renamed source");
    assert_eq!(state["title"]["status"], "synced");
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let fork_id = uuid::Uuid::new_v4().to_string();
    let p = json!({"target":target,"title":"Fork source"});
    let fork = control_receipt(&mut c, &identity, &fork_id, "session.codex.fork", p.clone()).await;
    assert_eq!(fork["phase"], "confirmed", "{fork}");
    let fork_sid = fork["result"]["newSessionId"].as_str().unwrap();
    assert_eq!(
        control_receipt(&mut c, &identity, &fork_id, "session.codex.fork", p).await,
        fork
    );
    let branched = c
        .call("session.get", json!({"sessionId":fork_sid}))
        .await
        .unwrap();
    assert_eq!(branched["desired"], "stopped");
    assert_eq!(branched["agentSessionId"], fork["result"]["nativeThreadId"]);
    assert_eq!(
        c.call("session.get", json!({"sessionId":sid}))
            .await
            .unwrap()["agentSessionId"],
        original
    );
    assert_eq!(native_calls(&fixture, "turn/start").len(), turns);
    assert_eq!(native_calls(&fixture, "thread/start").len(), starts);
    assert_eq!(native_calls(&fixture, "thread/resume").len(), resumes);
    assert_eq!(native_calls(&fixture, "thread/fork").len(), 1);
    assert!(
        registry(&fixture.data).is_empty(),
        "every metadata child registered and reaped"
    );
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controls_unknown_settings_and_fork_never_retry_on_manual_reopen() {
    let fixture = Fixture::new();
    let mut options = fixture.options("ready");
    let o = options.orchestra.as_mut().unwrap();
    o.claude = ClaudeProgram::Disabled;
    o.codex = CodexProgram::Path(artifact("fake_codex"));
    o.codex_env = BTreeMap::from([
        ("FAKE_CODEX_LOG".into(), fixture.log.display().to_string()),
        ("FAKE_CODEX_MODE".into(), "settings_exit,fork_exit".into()),
    ]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut c = Client::connect(&fixture.data).await;
    let hello = c.call("system.hello", json!({})).await.unwrap();
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("session.codex"))
    );
    let identity = hello["runtimeIdentity"].clone();
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Unknown controls","workingDirectory":fixture.work}),
        )
        .await
        .unwrap();
    let row=c.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","title":"Unknown source","brief":"Original task only once","isolation":false})).await.unwrap();
    let sid = row["id"].as_str().unwrap();
    wait("original task", || {
        turn_count(&fixture, "Original task only once") == 1
    })
    .await;
    let original = c
        .call("session.get", json!({"sessionId":sid}))
        .await
        .unwrap()["agentSessionId"]
        .clone();
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let params = json!({"target":target,"patch":{"fast":true}});
    let result = control_receipt(
        &mut c,
        &identity,
        &id,
        "session.codex.configure",
        params.clone(),
    )
    .await;
    assert_eq!(result["phase"], "unknown", "{result}");
    c.stopped(sid).await;
    let frozen = control_receipt(&mut c, &identity, &id, "session.codex.configure", params).await;
    assert_eq!(frozen, result);
    assert_eq!(native_calls(&fixture, "thread/settings/update").len(), 1);
    c.call("session.resume", json!({"sessionId":sid}))
        .await
        .unwrap();
    wait("explicit native resume", || {
        native_calls(&fixture, "thread/resume").len() == 1
    })
    .await;
    assert_eq!(
        native_calls(&fixture, "thread/settings/update").len(),
        1,
        "manual reopen must not retry an Unknown configure"
    );
    assert_eq!(
        native_calls(&fixture, "thread/start").len(),
        1,
        "strict resume never substitutes a new thread"
    );
    assert_eq!(
        turn_count(&fixture, "Original task only once"),
        1,
        "manual reopen must not replay original body"
    );
    let observed = c
        .call("session.codex.inspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(observed["settingsStatus"], "unknown");
    assert_eq!(
        c.call("session.get", json!({"sessionId":sid}))
            .await
            .unwrap()["agentSessionId"],
        original
    );
    c.call(
        "session.stop",
        json!({"sessionId":sid,"reason":"fork unknown phase"}),
    )
    .await
    .unwrap();
    c.stopped(sid).await;
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let fork_id = uuid::Uuid::new_v4().to_string();
    let p = json!({"target":target,"title":"Unknown fork"});
    let result =
        control_receipt(&mut c, &identity, &fork_id, "session.codex.fork", p.clone()).await;
    assert_eq!(result["phase"], "unknown", "{result}");
    let fork_sid = result["result"]["newSessionId"].as_str().unwrap();
    assert_eq!(
        control_receipt(&mut c, &identity, &fork_id, "session.codex.fork", p).await,
        result
    );
    let before = native_calls(&fixture, "initialize").len();
    assert!(
        c.call("session.resume", json!({"sessionId":fork_sid}))
            .await
            .is_err(),
        "unknown fork must refuse before any spawn"
    );
    assert_eq!(native_calls(&fixture, "initialize").len(), before);
    assert_eq!(native_calls(&fixture, "thread/fork").len(), 1);
    assert_eq!(
        c.call("session.get", json!({"sessionId":fork_sid}))
            .await
            .unwrap()["desired"],
        "stopped"
    );
    assert!(registry(&fixture.data).is_empty());
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // A fresh Core must preserve the original uncertain fork without replacement thread/start.
    let mut options = fixture.options("ready");
    let o = options.orchestra.as_mut().unwrap();
    o.claude = ClaudeProgram::Disabled;
    o.codex = CodexProgram::Path(artifact("fake_codex"));
    o.codex_env = BTreeMap::from([
        ("FAKE_CODEX_LOG".into(), fixture.log.display().to_string()),
        ("FAKE_CODEX_MODE".into(), "settings_exit,fork_exit".into()),
    ]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(
        fixture.data.clone(),
        options,
        async {
            let _ = stop_rx.await;
        },
    ));
    let mut c = Client::connect(&fixture.data).await;
    let reloaded = c
        .call(
            "command.lookup",
            json!({"requestId":fork_id,"dbId":identity["dbId"]}),
        )
        .await
        .unwrap();
    let mut historical = result.clone();
    historical["historical"] = json!(true);
    assert_eq!(
        reloaded, historical,
        "original fork UUID/result survives actual Core restart; only original receipt historical projection changes"
    );
    assert!(
        c.call("session.resume", json!({"sessionId":fork_sid}))
            .await
            .is_err()
    );
    let unknown_target = c
        .call("session.controlTarget", json!({"sessionId":fork_sid}))
        .await
        .unwrap();
    let new_identity = c.call("system.hello", json!({})).await.unwrap()["runtimeIdentity"].clone();
    let title_request = uuid::Uuid::new_v4().to_string();
    let denied = control_receipt(
        &mut c,
        &new_identity,
        &title_request,
        "session.codex.titleSet",
        json!({"target":unknown_target,"title":"No native identity"}),
    )
    .await;
    assert_eq!(
        denied["phase"], "suppressed",
        "a missing native source must settle before any provider write"
    );

    assert_eq!(
        native_calls(&fixture, "initialize").len(),
        before,
        "restart/unknown resume cannot create a replacement child"
    );
    assert_eq!(native_calls(&fixture, "thread/fork").len(), 1);
    assert_eq!(native_calls(&fixture, "thread/start").len(), 1);
    assert!(registry(&fixture.data).is_empty());
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

async fn metadata_pending_case(stop_first: bool, public_stop: bool) {
    let f = Fixture::new();
    let gate = f.root.path().join("metadata.hold");
    let mut options = f.options("ready");
    let o = options.orchestra.as_mut().unwrap();
    o.claude = ClaudeProgram::Disabled;
    o.codex = CodexProgram::Path(artifact("fake_codex"));
    o.codex_env = BTreeMap::from([
        ("FAKE_CODEX_LOG".into(), f.log.display().to_string()),
        ("FAKE_CODEX_INIT_GATE".into(), gate.display().to_string()),
    ]);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(f.data.clone(), options, async {
        let _ = stop_rx.await;
    }));
    let mut c = Client::connect(&f.data).await;
    let identity = c.call("system.hello", json!({})).await.unwrap()["runtimeIdentity"].clone();
    let crew = c
        .call(
            "crew.create",
            json!({"title":"metadata order","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let row=c.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","title":"initial","brief":"","isolation":false})).await.unwrap();
    let sid = row["id"].as_str().unwrap();
    wait("initial native name", || {
        native_calls(&f, "thread/name/set").len() == 1
    })
    .await;
    c.call(
        "session.stop",
        json!({"sessionId":sid,"reason":"metadata seam"}),
    )
    .await
    .unwrap();
    c.stopped(sid).await;
    std::fs::write(&gate, b"hold only first metadata initialize").unwrap();
    let first_id = uuid::Uuid::new_v4().to_string();
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    let dispatched=c.call("command.submit",json!({"requestId":first_id,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.codex.titleSet","params":{"target":target,"title":"first slow"}}})).await.unwrap();
    assert_eq!(dispatched["phase"], "dispatched");
    wait("first metadata child held", || {
        gate.with_extension("entered").exists()
    })
    .await;
    if !stop_first {
        for method in [
            "session.codex.inspect",
            "session.codex.history",
            "session.codex.models",
            "session.codex.tools",
            "session.usage",
        ] {
            assert!(
                c.call(method, json!({"sessionId":sid}))
                    .await
                    .unwrap()
                    .is_object(),
                "ordinary reads remain available while a metadata mutation is pending: {method}"
            );
        }
    }
    let second = if stop_first && public_stop {
        let target = c
            .call("session.controlTarget", json!({"sessionId":sid}))
            .await
            .unwrap();
        let stopped = control_receipt(
            &mut c,
            &identity,
            &uuid::Uuid::new_v4().to_string(),
            "session.stop",
            json!({"target":target,"reason":"cancel metadata through normal public Stop"}),
        )
        .await;
        // A stopped source has no new owned birth to certify. The original
        // physical Stop receipt may be Unknown; do not weaken it for this test.
        stopped
    } else if stop_first {
        c.call(
            "session.stop",
            json!({"sessionId":sid,"reason":"cancel held metadata before native write"}),
        )
        .await
        .unwrap();
        json!(null)
    } else {
        let target = c
            .call("session.controlTarget", json!({"sessionId":sid}))
            .await
            .unwrap();
        control_receipt(
            &mut c,
            &identity,
            &uuid::Uuid::new_v4().to_string(),
            "session.codex.titleSet",
            json!({"target":target,"title":"second fast"}),
        )
        .await
    };
    std::fs::write(gate.with_extension("release"), b"release").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    let first = loop {
        let r = c
            .call(
                "command.lookup",
                json!({"requestId":first_id,"dbId":identity["dbId"]}),
            )
            .await
            .unwrap();
        if r["phase"] != "dispatched" {
            break r;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let inspect = c
        .call("session.codex.inspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    let names: Vec<_> = native_calls(&f, "thread/name/set")
        .into_iter()
        .map(|r| r["params"]["name"].clone())
        .collect();
    eprintln!(
        "metadata stop={stop_first} first={first} second={second} native_order={names:?} title={}",
        inspect["title"]
    );
    assert!(registry(&f.data).is_empty());
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if stop_first {
        if public_stop {
            assert!(
                matches!(second["phase"].as_str(), Some("confirmed" | "unknown")),
                "Stop must have dispatched, never silently bypassed: {second}"
            );
        }
        assert_eq!(
            names,
            vec![json!("initial")],
            "durable explicit Stop must revoke stopped metadata before native name write"
        );
        assert_eq!(first["phase"], "unknown");
        assert_ne!(inspect["title"]["status"], "synced");
    } else {
        assert_eq!(
            second["phase"], "suppressed",
            "a second metadata mutation must not race a still-owned first child"
        );
        assert_eq!(names, vec![json!("initial"), json!("first slow")]);
        assert_eq!(inspect["title"]["native"], "first slow");
        assert_eq!(inspect["title"]["status"], "synced");
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_pending_title_cannot_be_overtaken_by_a_second_child() {
    metadata_pending_case(false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_pending_stop_revokes_before_native_write() {
    metadata_pending_case(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_pending_public_stop_revokes_before_native_write() {
    metadata_pending_case(true, true).await;
}

// The existing full serve/fake protocol seam; no provider account or project.
async fn completion_receipt(c:&mut Client,identity:&Value,request:&str)->Value {
    let until=tokio::time::Instant::now()+Duration::from_secs(8);
    loop {
        let receipt=c.call("command.lookup",json!({"dbId":identity["dbId"],"requestId":request})).await.unwrap();
        if !matches!(receipt["phase"].as_str(),Some("admitted"|"dispatched")){return receipt}
        assert!(tokio::time::Instant::now()<until,"original command did not settle: {receipt}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn completion_command(c:&mut Client,identity:&Value,sid:&str,operation:&str,mut params:Value)->Value {
    let request=uuid::Uuid::new_v4().to_string();
    params["target"]=c.call("session.controlTarget",json!({"sessionId":sid})).await.unwrap();
    c.call("command.submit",json!({"requestId":request,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":operation,"params":params}})).await.unwrap();
    completion_receipt(c,identity,&request).await
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn completion_public_native_input_profile_and_history_have_one_real_callable_entry(){
    use base64::{Engine,engine::general_purpose::STANDARD};
    let f=Fixture::new();let mut options=f.options("ready");let o=options.orchestra.as_mut().unwrap();
    o.claude=ClaudeProgram::Disabled;o.codex=CodexProgram::Path(artifact("fake_codex"));
    o.codex_env=BTreeMap::from([("FAKE_CODEX_LOG".into(),f.log.display().to_string()),("FAKE_CODEX_MODE".into(),"completion".into())]);
    let(stop_tx,stop_rx)=tokio::sync::oneshot::channel();let server=tokio::spawn(crewd_server::serve_with(f.data.clone(),options,async{let _=stop_rx.await;}));
    let mut c=Client::connect(&f.data).await;
    let identity=c.call("system.hello",json!({})).await.unwrap()["runtimeIdentity"].clone();
    let crew=c.call("crew.create",json!({"title":"Native inputs history","workingDirectory":f.work})).await.unwrap();
    let catalog=c.call("session.codex.tools",json!({"crewId":crew["id"]})).await.unwrap();
    let selected_skill=json!({"name":catalog["skills"][0]["skills"][0]["name"],"path":catalog["skills"][0]["skills"][0]["path"]});
    let first_profiles=c.call("session.codex.permissionProfiles",json!({"crewId":crew["id"]})).await.unwrap();assert_eq!(first_profiles["items"][0]["allowed"],true);
    let sessions_before=c.call("session.list",json!({"crewId":crew["id"]})).await.unwrap();assert!(sessions_before.as_array().unwrap().is_empty());
    assert!(c.call("session.codex.tools",json!({"crewId":crew["id"],"sessionId":"foreign"})).await.is_err());
    assert!(c.call("session.codex.permissionProfiles",json!({})).await.is_err());
    let bytes=b"Original separate attached text";let prepared_request=uuid::Uuid::new_v4().to_string();
    let prepared=c.call("session.codex.inputPrepare",json!({"requestId":prepared_request,"crewId":crew["id"],"kind":"text","filename":"notes.txt","mimeType":"text/plain","sha256":crewd_store::codex_inputs::hash(bytes),"bytesBase64":STANDARD.encode(bytes)})).await.unwrap();
    assert_eq!(prepared["operation"],"prepared");assert_eq!(native_calls(&f,"thread/start").len(),0,"prepare/catalog never opens native thread");assert_eq!(native_calls(&f,"turn/start").len(),0);
    assert_eq!(c.call("session.codex.inputLookup",json!({"crewId":crew["id"],"requestId":prepared_request})).await.unwrap()["result"],prepared);
    assert!(c.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","brief":"Not dispatched legacy input","isolation":false,"codexInputs":{"inputs":[prepared["input"]]}})).await.is_err());
    let request=uuid::Uuid::new_v4().to_string();
    let task="Implement this exact task.\n你是 remains Human text.";
    c.call("command.submit",json!({"requestId":request,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.start","params":{"crewId":crew["id"],"role":"captain","executor":"codex","title":"Native first input","brief":task,"isolation":false,"codexOptions":{"permissions":"synthetic-profile"},"codexInputs":{"inputs":[prepared["input"]],"skills":[selected_skill]}}}})).await.unwrap();
    let receipt=completion_receipt(&mut c,&identity,&request).await;assert_eq!(receipt["phase"],"launched","{receipt}");
    let sessions=c.call("session.list",json!({"crewId":crew["id"]})).await.unwrap();
    let sid=sessions.as_array().unwrap()[0]["id"].as_str().unwrap().to_owned();
    wait("original native first input",||native_calls(&f,"turn/start").len()==1).await;
    let sent=native_calls(&f,"turn/start")[0].clone();assert_eq!(sent["params"]["input"][0]["text"],task);assert_eq!(sent["params"]["input"].as_array().unwrap().len(),3);
    assert_eq!(native_calls(&f,"permissionProfile/list").len(),2);
    assert_eq!(native_calls(&f,"thread/settings/update")[0]["params"]["permissions"],"synthetic-profile");
    let profile=c.call("session.codex.permissionProfiles",json!({"sessionId":sid})).await.unwrap();assert_eq!(profile["items"][0]["id"],"synthetic-profile");
    let before=std::fs::read_to_string(&f.log).unwrap();let methods:Vec<_>=before.lines().filter_map(|s|serde_json::from_str::<Value>(s).ok()).filter_map(|v|v["method"].as_str().map(str::to_owned)).collect();
    assert!(methods.iter().position(|s|s=="thread/settings/update").unwrap()<methods.iter().position(|s|s=="turn/start").unwrap());
    assert!(c.call("session.codex.inputDiscard",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew["id"],"input":prepared["input"]})).await.is_err());
    // Wait native completion before capturing the existing exact idle target.
    let until=tokio::time::Instant::now()+Duration::from_secs(5);loop{if c.call("session.codex.inspect",json!({"sessionId":sid})).await.unwrap()["working"]==false{break}assert!(tokio::time::Instant::now()<until);tokio::task::yield_now().await;}
    let reverted=completion_command(&mut c,&identity,&sid,"session.codex.revert",json!({"beforeTurnId":"original-turn"})).await;assert_eq!(reverted["phase"],"confirmed","{reverted}");
    let refused=completion_command(&mut c,&identity,&sid,"session.codex.archive",json!({})).await;assert_eq!(refused["phase"],"suppressed");assert_eq!(native_calls(&f,"thread/archive").len(),0);
    c.call("session.stop",json!({"sessionId":sid,"reason":"explicit history management"})).await.unwrap();c.stopped(&sid).await;
    for(operation,status)in[("session.codex.archive","archived"),("session.codex.unarchive","active"),("session.codex.delete","deleted")]{
        let before=native_calls(&f,"thread/resume").len();
        let receipt=completion_command(&mut c,&identity,&sid,operation,if operation=="session.codex.delete"{json!({"confirmNativeHistoryDelete":true})}else{json!({})}).await;
        assert_eq!(receipt["phase"],"confirmed","{receipt}");assert_eq!(c.call("session.codex.inspect",json!({"sessionId":sid})).await.unwrap()["nativeHistory"]["status"],status);
        assert_eq!(native_calls(&f,"thread/resume").len(),before,"history operation cannot resume");
        if status=="archived"||status=="deleted"{assert!(c.call("session.resume",json!({"sessionId":sid})).await.is_err());}
    }
    assert!(c.call("session.get",json!({"sessionId":sid})).await.is_ok(),"PCW session survives native delete");
    stop_tx.send(()).unwrap();tokio::time::timeout(Duration::from_secs(15),server).await.unwrap().unwrap().unwrap();assert!(registry(&f.data).is_empty());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn completion_lost_history_ack_reopens_original_unknown_without_native_replay(){
    let f=Fixture::new();let mut sid=String::new();let mut original_identity=Value::Null;let mut request=String::new();
    for round in 0..2 {
        let mut options=f.options("ready");let o=options.orchestra.as_mut().unwrap();o.claude=ClaudeProgram::Disabled;o.codex=CodexProgram::Path(artifact("fake_codex"));
        o.codex_env=BTreeMap::from([("FAKE_CODEX_LOG".into(),f.log.display().to_string()),("FAKE_CODEX_MODE".into(),"completion,history_exit".into())]);
        let(stop_tx,stop_rx)=tokio::sync::oneshot::channel();let server=tokio::spawn(crewd_server::serve_with(f.data.clone(),options,async{let _=stop_rx.await;}));
        let mut c=Client::connect(&f.data).await;
        if round==0 {
            original_identity=c.call("system.hello",json!({})).await.unwrap()["runtimeIdentity"].clone();
            let crew=c.call("crew.create",json!({"title":"Original uncertain history","workingDirectory":f.work})).await.unwrap();
            let session=c.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","title":"Original history","brief":"","isolation":false})).await.unwrap();sid=session["id"].as_str().unwrap().to_owned();
            wait("original empty runtime ready",||native_calls(&f,"thread/start").len()==1).await;
            c.call("session.stop",json!({"sessionId":sid,"reason":"explicit native history"})).await.unwrap();c.stopped(&sid).await;
            request=uuid::Uuid::new_v4().to_string();let target=c.call("session.controlTarget",json!({"sessionId":sid})).await.unwrap();
            let original=json!({"requestId":request,"dbId":original_identity["dbId"],"runtimeBindingId":original_identity["runtimeBindingId"],"command":{"operation":"session.codex.archive","params":{"target":target}}});
            c.call("command.submit",original.clone()).await.unwrap();let receipt=completion_receipt(&mut c,&original_identity,&request).await;assert_eq!(receipt["phase"],"unknown","{receipt}");
            assert_eq!(native_calls(&f,"thread/archive").len(),1);
            assert_eq!(c.call("command.submit",original.clone()).await.unwrap()["phase"],"unknown");
            assert_eq!(native_calls(&f,"thread/archive").len(),1);
            // Uncertain native history affects this source's mutation only; crew catalogs remain usable.
            assert!(c.call("session.codex.permissionProfiles",json!({"crewId":crew["id"]})).await.is_ok());
        } else {
            assert!(c.call("session.resume",json!({"sessionId":sid})).await.is_err());
            let receipt=c.call("command.lookup",json!({"dbId":original_identity["dbId"],"requestId":request})).await.unwrap();assert_eq!(receipt["phase"],"unknown");assert_eq!(receipt["historical"],true);
            let inspection=c.call("session.codex.inspect",json!({"sessionId":sid})).await.unwrap();assert_eq!(inspection["nativeHistory"]["status"],"unknown");assert_eq!(inspection["running"],false);
        }
        assert_eq!(native_calls(&f,"thread/resume").len(),0);assert_eq!(native_calls(&f,"turn/start").len(),0);
        stop_tx.send(()).unwrap();tokio::time::timeout(Duration::from_secs(15),server).await.unwrap().unwrap().unwrap();assert!(registry(&f.data).is_empty());
    }
    assert_eq!(native_calls(&f,"thread/start").len(),1);assert_eq!(native_calls(&f,"thread/archive").len(),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="requires exact approved immutable old19 helper path and hash"]
async fn completion_exact_old19_writer_refuses20_before_spawn_and_preserves_owned_inputs(){
    use base64::{Engine,engine::general_purpose::STANDARD};
    use crewd_proto::codex_inputs::*;
    let binary=std::path::PathBuf::from(std::env::var("PCW_CODEX_OLD19_CORE").unwrap());let expected=std::env::var("PCW_CODEX_OLD19_SHA256").unwrap();
    assert_eq!(crewd_store::codex_inputs::hash(&std::fs::read(&binary).unwrap()),expected);
    let f=Fixture::new();std::fs::create_dir_all(&f.data).unwrap();
    let mut store=crewd_store::Store::open_data_dir(&f.data).unwrap();
    store.conn().execute("INSERT INTO crews(id,title,working_directory,created_at,updated_at)VALUES('crew','Old writer controlled source',?1,'t','t')",[f.work.to_string_lossy().as_ref()]).unwrap();
    store.conn().execute("INSERT INTO sessions(id,crew_id,role,executor,agent_session_id,working_directory,desired,updated_at)VALUES('original-session','crew','worker','codex','original-native-thread',?1,'stopped','t')",[f.work.to_string_lossy().as_ref()]).unwrap();
    let owner=crewd_store::codex_inputs::Owner{db_id:crewd_store::meta_get(store.conn(),"dbId").unwrap().unwrap(),principal:format!("local-human:{}",unsafe{libc::geteuid()})};
    let body=b"Durable original owned input";let request=uuid::Uuid::new_v4().to_string();
    let params=CodexInputPrepareParams{request_id:request.clone(),crew_id:"crew".into(),session_id:Some("original-session".into()),kind:CodexInputKind::Text,filename:"original.txt".into(),mime_type:"text/plain".into(),sha256:crewd_store::codex_inputs::hash(body),bytes_base64:STANDARD.encode(body)};
    let prepared=store.with_tx(|c|crewd_store::codex_inputs::prepare(c,&owner,&params,body,"t")).unwrap();
    let before_meta=crewd_store::meta_get(store.conn(),"codex.controls.v1/original-session").unwrap().unwrap();
    assert_eq!(serde_json::from_str::<Value>(&before_meta).unwrap()["version"],2);drop(store);
    let log=std::fs::File::create(f.root.path().join("old19-terminal.log")).unwrap();
    let mut old=tokio::process::Command::new(&binary).arg("--data-dir").arg(&f.data).arg("serve").arg("--codex").arg(artifact("fake_codex")).arg("--codex-env").arg(format!("FAKE_CODEX_LOG={}",f.log.display())).stdout(log.try_clone().unwrap()).stderr(log).kill_on_drop(true).spawn().unwrap();
    let status=tokio::time::timeout(Duration::from_secs(10),old.wait()).await.unwrap().unwrap();
    assert!(!status.success(),"old19 must refuse20 before socket or native startup");
    let terminal=std::fs::read_to_string(f.root.path().join("old19-terminal.log")).unwrap();assert!(terminal.contains("20")&&terminal.contains("19"),"{terminal}");
    assert!(!f.data.join("crewd.sock").exists());
    assert_eq!(native_calls(&f,"initialize").len(),0);assert_eq!(native_calls(&f,"thread/resume").len(),0);assert_eq!(native_calls(&f,"turn/start").len(),0);
    let store=crewd_store::Store::open_data_dir(&f.data).unwrap();assert_eq!(crewd_store::meta_get(store.conn(),"codex.controls.v1/original-session").unwrap().unwrap(),before_meta);
    assert_eq!(crewd_store::codex_inputs::lookup(store.conn(),&owner,&CodexInputLookupParams{crew_id:"crew".into(),request_id:request}).unwrap(),CodexInputLookup::Known{result:prepared});
    assert_eq!(store.conn().query_row("SELECT body FROM codex_native_inputs",[],|r|r.get::<_,Vec<u8>>(0)).unwrap(),body);
    assert_eq!(store.conn().query_row("SELECT agent_session_id FROM sessions WHERE id='original-session'",[],|r|r.get::<_,String>(0)).unwrap(),"original-native-thread");
    assert_eq!(crewd_store::codex_inputs::hash(&std::fs::read(&binary).unwrap()),expected);
    println!("PASS exact old19 refused20 before all native spawn/input, original binding/metadata/input/receipt unchanged");
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn completion_crew_profile_result_cannot_be_laundered_into_tools_shape(){
    let f=Fixture::new();let mut options=f.options("ready");let o=options.orchestra.as_mut().unwrap();o.claude=ClaudeProgram::Disabled;o.codex=CodexProgram::Path(artifact("fake_codex"));o.codex_env=BTreeMap::from([("FAKE_CODEX_LOG".into(),f.log.display().to_string()),("FAKE_CODEX_MODE".into(),"completion,profile_wrong_shape".into())]);
    let(stop_tx,stop_rx)=tokio::sync::oneshot::channel();let server=tokio::spawn(crewd_server::serve_with(f.data.clone(),options,async{let _=stop_rx.await;}));let mut c=Client::connect(&f.data).await;
    let crew=c.call("crew.create",json!({"title":"Closed catalog projection","workingDirectory":f.work})).await.unwrap();
    assert!(c.call("session.codex.permissionProfiles",json!({"crewId":crew["id"]})).await.is_err(),"profile request must not accept a tools-shaped native response");
    let tools=c.call("session.codex.tools",json!({"crewId":crew["id"]})).await.unwrap();assert!(tools["skills"].is_array());assert!(tools.get("items").is_none());assert!(native_calls(&f,"thread/start").is_empty());assert!(native_calls(&f,"turn/start").is_empty());
    stop_tx.send(()).unwrap();tokio::time::timeout(Duration::from_secs(15),server).await.unwrap().unwrap().unwrap();assert!(registry(&f.data).is_empty());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn completion_native_first_inputs_require_nonblank_original_task_before_any_admission(){
    use base64::{Engine,engine::general_purpose::STANDARD};
    let f=Fixture::new();let mut options=f.options("ready");let o=options.orchestra.as_mut().unwrap();
    o.claude=ClaudeProgram::Disabled;o.codex=CodexProgram::Path(artifact("fake_codex"));
    o.codex_env=BTreeMap::from([("FAKE_CODEX_LOG".into(),f.log.display().to_string()),("FAKE_CODEX_MODE".into(),"completion".into())]);
    let(stop_tx,stop_rx)=tokio::sync::oneshot::channel();let server=tokio::spawn(crewd_server::serve_with(f.data.clone(),options,async{let _=stop_rx.await;}));
    let mut c=Client::connect(&f.data).await;let identity=c.call("system.hello",json!({})).await.unwrap()["runtimeIdentity"].clone();
    let mut violations=vec![];
    for (brief,skill_only) in [("",false),(" \n\t",false),("",true),(" \n\t",true)]{
        let crew=c.call("crew.create",json!({"title":"No orphan first input","workingDirectory":f.work})).await.unwrap();
        let original=uuid::Uuid::new_v4().to_string();let bytes=b"Unused original input survives refusal";
        let prepared=c.call("session.codex.inputPrepare",json!({"requestId":original,"crewId":crew["id"],"kind":"text","filename":"original.txt","mimeType":"text/plain","sha256":crewd_store::codex_inputs::hash(bytes),"bytesBase64":STANDARD.encode(bytes)})).await.unwrap();
        let selection=if skill_only{json!({"skills":[{"name":"synthetic-skill","path":f.work.canonicalize().unwrap().join("SKILL.md")} ]})}else{json!({"inputs":[prepared["input"]]})};
        let request=uuid::Uuid::new_v4().to_string();
        let submitted=c.call("command.submit",json!({"requestId":request,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.start","params":{"crewId":crew["id"],"role":"worker","executor":"codex","title":"Input","brief":brief,"isolation":false,"codexInputs":selection}}})).await;
        let receipt=if submitted.is_ok(){completion_receipt(&mut c,&identity,&request).await}else{c.call("command.lookup",json!({"dbId":identity["dbId"],"requestId":request})).await.unwrap()};
        let sessions=c.call("session.list",json!({"crewId":crew["id"]})).await.unwrap();
        let db=rusqlite::Connection::open_with_flags(f.data.join(crewd_store::DB_FILE),rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let state:String=db.query_row("SELECT state FROM codex_native_inputs WHERE id=?1",[prepared["input"]["inputId"].as_str().unwrap()],|r|r.get(0)).unwrap();drop(db);
        let lookup=c.call("session.codex.inputLookup",json!({"crewId":crew["id"],"requestId":original})).await.unwrap();assert_eq!(lookup["result"],prepared);
        println!("blank={brief:?} skill_only={skill_only} submit_error={} receipt={receipt} sessions={} ref_state={state} turn_count={}",submitted.is_err(),sessions.as_array().unwrap().len(),native_calls(&f,"turn/start").len());
        if submitted.is_ok()||!receipt.is_null()||!sessions.as_array().unwrap().is_empty()||state!="prepared"{violations.push(format!("blank={brief:?} skill_only={skill_only} admitted or consumed"));}
    }
    if !native_calls(&f,"initialize").is_empty()||!native_calls(&f,"thread/start").is_empty()||!native_calls(&f,"turn/start").is_empty(){violations.push("blank native input physically spawned".into());}
    // No selection still permits the original intentionally empty session.
    let crew=c.call("crew.create",json!({"title":"Original empty session","workingDirectory":f.work})).await.unwrap();let request=uuid::Uuid::new_v4().to_string();
    c.call("command.submit",json!({"requestId":request,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.start","params":{"crewId":crew["id"],"role":"worker","executor":"codex","title":"Empty","brief":"","isolation":false}}})).await.unwrap();
    assert_eq!(completion_receipt(&mut c,&identity,&request).await["phase"],"launched");
    assert_eq!(native_calls(&f,"turn/start").len(),0);
    stop_tx.send(()).unwrap();tokio::time::timeout(Duration::from_secs(15),server).await.unwrap().unwrap().unwrap();assert!(registry(&f.data).is_empty());
    assert!(violations.is_empty(),"{}",violations.join("; "));
}
