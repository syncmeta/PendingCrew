//! Real Unix transport with an intentionally unread submit response. No models.
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::oneshot,
};

async fn start(dir: &Path) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = oneshot::channel();
    let dir = dir.to_owned();
    let socket = dir.join("crewd.sock");
    let task = tokio::spawn(async move {
        crewd_server::serve(dir, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            return (tx, task);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}
async fn call(stream: &mut BufReader<UnixStream>, method: &str, params: Value) -> Value {
    let request = json!({"id":1,"method":method,"params":params});
    stream
        .get_mut()
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert!(frame.get("error").is_none(), "{frame}");
    frame["result"].clone()
}
async fn connect(dir: &Path) -> BufReader<UnixStream> {
    BufReader::new(UnixStream::connect(dir.join("crewd.sock")).await.unwrap())
}
#[tokio::test]
async fn recovery_inspection_is_read_only_and_requires_an_original_session() {
    let dir = tempfile::tempdir().unwrap();
    let (stop, server) = start(dir.path()).await;
    let mut client = connect(dir.path()).await;
    let hello = call(&mut client, "system.hello", json!({})).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("session.claude.recovery"))
    );
    drop(client);
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[path = "support/t11.rs"]
mod support;
async fn configured_start(
    f: &support::Fixture,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    support::Client,
) {
    let (tx, rx) = oneshot::channel();
    let server = tokio::spawn(crewd_server::serve_with(
        f.data.clone(),
        f.options("ready"),
        async {
            let _ = rx.await;
        },
    ));
    let client = support::Client::connect(&f.data).await;
    (tx, server, client)
}
async fn configured_stop(
    tx: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
fn resolve_packet(hello: &Value, inspection: &Value, id: &str) -> Value {
    json!({"requestId":id,"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.claude.recoveryResolve","params":{"target":inspection["target"],"originalAgentSessionId":inspection["originalAgentSessionId"],"evidenceToken":inspection["evidenceToken"],"originalRequestIds":inspection["unresolved"].as_array().unwrap().iter().map(|r|r["requestId"].clone()).collect::<Vec<_>>()}}})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn original_unknowns_disposition_restart_competition_and_fresh_evidence_gate() {
    use crewd_proto::commands::*;
    use crewd_store::command_receipts as receipts;
    let f = support::Fixture::new();
    let (tx, server, mut c) = configured_start(&f).await;
    let hello = c.call("system.hello", json!({})).await.unwrap();
    let crew = c
        .call(
            "crew.create",
            json!({"title":"Recovery fixture","workingDirectory":f.work}),
        )
        .await
        .unwrap();
    let session=c.call("session.start",json!({"crewId":crew["id"],"role":"worker","executor":"claude","brief":"ONLY_ORIGINAL_TASK","isolation":false})).await.unwrap();
    let sid = session["id"].as_str().unwrap().to_owned();
    support::wait("original delivered", || f.count("ONLY_ORIGINAL_TASK") == 1).await;
    c.call("session.stop", json!({"sessionId":sid,"reason":"fixture"}))
        .await
        .unwrap();
    c.stopped(&sid).await;
    let target = c
        .call("session.controlTarget", json!({"sessionId":sid}))
        .await
        .unwrap();
    drop(c);
    configured_stop(tx, server).await;
    // An owned, closed fixture database models two pre-contract unresolved resumes.
    // All subsequent inspection/disposition/recovery uses actual Unix RPC.
    let store = crewd_store::Store::open(&f.data.join("crewd.db")).unwrap();
    let uid = std::process::Command::new("id").arg("-u").output().unwrap();
    let principal = format!(
        "local-human:{}",
        String::from_utf8(uid.stdout).unwrap().trim()
    );
    let mut ids = vec![
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    ];
    ids.sort();
    for id in &ids {
        let p:CommandSubmitParams=serde_json::from_value(json!({"requestId":id,"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.resume","params":target}})).unwrap();
        receipts::reserve(store.conn(), &principal, &p, "fixture-original").unwrap();
        receipts::transition(
            store.conn(),
            &principal,
            &p.db_id,
            id,
            (CommandPhase::Reserved, CommandPhase::Unknown),
            Some(&json!({"actualOutcome":"launch_failed_without_confirmed_birth"})),
            "fixture-failed",
        )
        .unwrap();
    }
    // A long-lived session's settled controls must not consume the recovery
    // blocker budget. The rest of this real-service test must still dispose
    // the two actual Unknowns and explicitly resume exactly once.
    for _ in 0..300 {
        let p: CommandSubmitParams = serde_json::from_value(json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.resume","params":target}})).unwrap();
        receipts::reserve(store.conn(), &principal, &p, "settled-fixture").unwrap();
        receipts::transition(
            store.conn(),
            &principal,
            &p.db_id,
            &p.request_id,
            (CommandPhase::Reserved, CommandPhase::Suppressed),
            Some(&json!({"effectsDispatched":false})),
            "settled-fixture",
        )
        .unwrap();
    }
    let originals: Vec<_> = ids
        .iter()
        .map(|id| {
            serde_json::to_value(
                receipts::lookup(
                    store.conn(),
                    &principal,
                    hello["runtimeIdentity"]["dbId"].as_str().unwrap(),
                    id,
                )
                .unwrap()
                .unwrap(),
            )
            .unwrap()
        })
        .collect();
    drop(store);
    let (tx, server, mut c) = configured_start(&f).await;
    let hello = c.call("system.hello", json!({})).await.unwrap();
    let inspection = c
        .call("session.claude.recoveryInspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(inspection["eligibility"], "eligible", "{inspection}");
    assert_eq!(inspection["unresolved"].as_array().unwrap().len(), 2);
    let before = std::fs::read(&f.log).unwrap();
    assert!(support::registry(&f.data).is_empty());
    let mut stale = resolve_packet(&hello, &inspection, &uuid::Uuid::new_v4().to_string());
    stale["command"]["params"]["evidenceToken"] = json!("changed");
    assert!(c.call("command.submit", stale).await.is_err());
    let moved = f.root.path().join("work-moved");
    std::fs::rename(&f.work, &moved).unwrap();
    assert!(
        c.call(
            "command.submit",
            resolve_packet(&hello, &inspection, &uuid::Uuid::new_v4().to_string())
        )
        .await
        .is_err(),
        "workdir disappeared after evidence"
    );
    std::fs::rename(&moved, &f.work).unwrap();
    let mut partial = resolve_packet(&hello, &inspection, &uuid::Uuid::new_v4().to_string());
    partial["command"]["params"]["originalRequestIds"] = json!([ids[0]]);
    assert!(
        c.call("command.submit", partial).await.is_err(),
        "cannot omit another Unknown"
    );
    let a = resolve_packet(&hello, &inspection, &uuid::Uuid::new_v4().to_string());
    let b = resolve_packet(&hello, &inspection, &uuid::Uuid::new_v4().to_string());
    let mut other = support::Client::connect(&f.data).await;
    let (ra, rb) = tokio::join!(
        c.call("command.submit", a.clone()),
        other.call("command.submit", b.clone())
    );
    assert_ne!(
        ra.is_ok(),
        rb.is_ok(),
        "only one disposition owns original evidence"
    );
    let (packet, receipt) = if let Ok(r) = ra {
        (a, r)
    } else {
        (b, rb.unwrap())
    };
    assert_eq!(receipt["phase"], "committed");
    assert_eq!(receipt["result"]["effectsDispatched"], false);
    assert_eq!(
        c.call("command.submit", packet.clone()).await.unwrap(),
        receipt,
        "same idempotent request"
    );
    assert_eq!(std::fs::read(&f.log).unwrap(), before);
    assert!(support::registry(&f.data).is_empty());
    let after = c
        .call("session.claude.recoveryInspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(after["eligibility"], "noUnresolvedRequests");
    drop(c);
    drop(other);
    configured_stop(tx, server).await;
    let store = crewd_store::Store::open(&f.data.join("crewd.db")).unwrap();
    for (id, original) in ids.iter().zip(&originals) {
        assert_eq!(
            serde_json::to_value(
                receipts::lookup(
                    store.conn(),
                    &principal,
                    hello["runtimeIdentity"]["dbId"].as_str().unwrap(),
                    id
                )
                .unwrap()
                .unwrap()
            )
            .unwrap(),
            *original,
            "original receipt byte fields unchanged"
        );
    }
    drop(store);
    let (tx, server, mut c) = configured_start(&f).await;
    let recovered = c
        .call(
            "command.lookup",
            json!({"requestId":packet["requestId"],"dbId":packet["dbId"]}),
        )
        .await
        .unwrap();
    assert_eq!(recovered["phase"], "committed");
    assert_eq!(recovered["historical"], true);
    assert_eq!(recovered["result"], receipt["result"]);
    let after = c
        .call("session.claude.recoveryInspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(after["eligibility"], "noUnresolvedRequests");
    assert_eq!(after["dispositions"].as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read(&f.log).unwrap(), before);
    assert!(support::registry(&f.data).is_empty());
    // A fresh explicit resume is separate from disposition; concurrent identical target
    // commands cannot start two physical processes.
    let hello = c.call("system.hello", json!({})).await.unwrap();
    let make_resume = || json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.resume","params":after["target"]}});
    let mut other = support::Client::connect(&f.data).await;
    let first_request = make_resume();
    let second_request = make_resume();
    let (first, second) = tokio::join!(
        c.call("command.submit", first_request.clone()),
        other.call("command.submit", second_request.clone())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    eprintln!("recovery competing resume submit receipts: first={first} second={second}");
    assert_ne!(
        first["phase"] == "suppressed",
        second["phase"] == "suppressed",
        "{first} {second}"
    );
    let (resumed_request, suppressed_request) = if first["phase"] == "suppressed" {
        (&second_request, &first_request)
    } else {
        (&first_request, &second_request)
    };
    // A registry entry precedes the Core's exact-generation Spawned observation.
    // Wait for the original public receipt before fixture cleanup; stopping an
    // unconfirmed launch would honestly add another Unknown to the later set.
    let launched = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let receipt = c
                .call(
                    "command.lookup",
                    json!({
                        "requestId":resumed_request["requestId"], "dbId":resumed_request["dbId"]
                    }),
                )
                .await
                .unwrap();
            assert_eq!(receipt["requestId"], resumed_request["requestId"]);
            assert_eq!(receipt["command"], resumed_request["command"]);
            match receipt["phase"].as_str() {
                Some("launched") => break receipt,
                Some("dispatched") => tokio::time::sleep(Duration::from_millis(25)).await,
                _ => panic!("original resume did not confirm its physical launch: {receipt}"),
            }
        }
    })
    .await
    .expect("original resume receipt must reach Launched before fixture cleanup");
    let suppressed = c
        .call(
            "command.lookup",
            json!({
                "requestId":suppressed_request["requestId"], "dbId":suppressed_request["dbId"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(suppressed["requestId"], suppressed_request["requestId"]);
    assert_eq!(suppressed["command"], suppressed_request["command"]);
    assert_eq!(suppressed["phase"], "suppressed", "{suppressed}");
    eprintln!(
        "recovery competing resume settled receipts: launched={launched} suppressed={suppressed}"
    );
    support::wait("one resumed process", || {
        support::registry(&f.data).len() == 1
    })
    .await;
    assert_eq!(
        f.count("ONLY_ORIGINAL_TASK"),
        1,
        "old task was never replayed"
    );
    c.call(
        "session.stop",
        json!({"sessionId":sid,"reason":"fixture cleanup"}),
    )
    .await
    .unwrap();
    c.stopped(&sid).await;
    drop(c);
    drop(other);
    configured_stop(tx, server).await;
    // New unresolved evidence after disposition must block both public controls
    // and the executor's legacy entry. Never widen a disposition to future UUIDs.
    let store = crewd_store::Store::open(&f.data.join("crewd.db")).unwrap();
    let p: CommandSubmitParams = serde_json::from_value(make_resume()).unwrap();
    receipts::reserve(store.conn(), &principal, &p, "new-fixture").unwrap();
    receipts::transition(
        store.conn(),
        &principal,
        &p.db_id,
        &p.request_id,
        (CommandPhase::Reserved, CommandPhase::Unknown),
        Some(&json!({"actualOutcome":"launch_failed_without_confirmed_birth"})),
        "new-failed",
    )
    .unwrap();
    drop(store);
    let (tx, server, mut c) = configured_start(&f).await;
    let hello = c.call("system.hello", json!({})).await.unwrap();
    let current = c
        .call("session.claude.recoveryInspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(current["eligibility"], "eligible");
    assert_eq!(
        current["unresolved"].as_array().unwrap().len(),
        1,
        "{current}"
    );
    let blocked=c.call("command.submit",json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":"session.resume","params":current["target"]}})).await.unwrap();
    assert_eq!(blocked["phase"], "suppressed");
    let _ = c.call("session.resume", json!({"sessionId":sid})).await;
    c.stopped(&sid).await;
    assert!(support::registry(&f.data).is_empty());
    assert_eq!(f.count("ONLY_ORIGINAL_TASK"), 1);
    drop(c);
    configured_stop(tx, server).await;
    // Imported historical state cannot carry forward the disposition authority.
    let store = crewd_store::Store::open(&f.data.join("crewd.db")).unwrap();
    receipts::restored(store.conn(), "fixture-restore").unwrap();
    drop(store);
    let (tx, server, mut c) = configured_start(&f).await;
    let restored = c
        .call("session.claude.recoveryInspect", json!({"sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(restored["eligibility"], "evidenceUnavailable");
    assert_eq!(restored["evidenceToken"], Value::Null);
    assert!(restored["dispositions"].as_array().unwrap().is_empty());
    assert!(support::registry(&f.data).is_empty());
    assert_eq!(f.count("ONLY_ORIGINAL_TASK"), 1);
    drop(c);
    configured_stop(tx, server).await;
}
