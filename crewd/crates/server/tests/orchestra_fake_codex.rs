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
