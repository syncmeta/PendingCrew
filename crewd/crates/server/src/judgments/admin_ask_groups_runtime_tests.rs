//! Actual loopback provider + captured Unix Human reads. No executor/model process.
use super::*;
use crate::AppState;
use crewd_collab::{asks, crews, sessions};
use crewd_proto::{Request, Role, judgments::*};
use serde_json::{Value, json};
#[expect(
    clippy::duplicate_mod,
    reason = "Use the real shared fake HTTP server in this independent optional judgment-family test"
)]
#[path = "../../../jev/tests/support/mod.rs"]
mod support;
use support::{FakeJev, Reply};

#[tokio::test]
async fn actual_settings_invalidation_reaches_captured_human_even_when_runner_is_unhealthy() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let f = Fixture::new().await;
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let state = f.state.clone();
    let task = tokio::spawn(crate::conn::run(state, server));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    let mut subscribe = serde_json::to_vec(&Request {
        id: 1,
        method: "events.subscribe".into(),
        params: json!({"topics":[crewd_proto::admin_ask_groups::TOPIC]}),
    })
    .unwrap();
    subscribe.push(b'\n');
    write.write_all(&subscribe).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(3), read.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let subscribed: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(subscribed["id"], 1);
    assert!(subscribed.get("error").is_none(), "{subscribed}");

    // Keep the failed-runner condition true across queue and writer fences;
    // a live periodic pump could otherwise repair the readiness flag mid-test.
    let pump = f.runtime.pump.lock().await.take().unwrap();
    pump.abort();
    assert!(pump.await.unwrap_err().is_cancelled());
    f.runtime.ask_groups_ready.store(false, Ordering::Release);
    assert!(!f.runtime.ask_groups_healthy());
    let committed = f
        .rpc(
            SETTINGS_SET,
            json!({
                "requestId":uuid::Uuid::new_v4().to_string(),"expectedRevision":null,
                "config":null,"credentialUpdate":{"action":"keep"}
            }),
        )
        .await
        .unwrap();
    assert_eq!(committed["outcome"], "committed");
    line.clear();
    tokio::time::timeout(Duration::from_secs(3), read.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"event":crewd_proto::admin_ask_groups::INVALIDATED,"data":{}})
    );
    assert!(!f.runtime.ask_groups_healthy());
    drop(write);
    drop(read);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    f.stop_db().await;
}

struct Fixture {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    runtime: Arc<super::super::Runtime>,
    crew: String,
    worker: String,
    captain: String,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().canonicalize().unwrap();
        let store = crewd_store::Store::open(&data.join(crewd_store::DB_FILE)).unwrap();
        assert!(
            crewd_store::admin_ask_groups::available(store.conn()).unwrap(),
            "requires the genuine protected schema-33 registration; never relabel a fixture"
        );
        let state = Arc::new(AppState::new(crewd_store::Db::spawn(store).unwrap()));
        state.judgment_test_http.store(true, Ordering::Release);
        let crew = state
            .collab(|tx| {
                crews::create(
                    tx,
                    crews::NewCrew {
                        title: "synthetic secretary source".into(),
                        working_directory: "/tmp/synthetic-never-launched".into(),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        let own = crew.clone();
        let captain = state
            .collab(move |tx| {
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: own,
                        role: Role::Captain,
                        executor: "claude".into(),
                        title: Some("secretary captain".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        let own = crew.clone();
        let worker = state
            .collab(move |tx| {
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: own,
                        role: Role::Worker,
                        executor: "claude".into(),
                        title: Some("secretary worker".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        state.admission.allow(&captain);
        state.admission.allow(&worker);
        let runtime = super::super::Runtime::start_with_admin_dispatch(&state, data, true, false)
            .await
            .unwrap()
            .unwrap();
        Self {
            dir,
            state,
            runtime,
            crew,
            worker,
            captain,
        }
    }
    async fn rpc(&self, method: &str, params: Value) -> Result<Value, crewd_proto::RpcError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let state = self.state.clone();
        let task = tokio::spawn(crate::conn::run(state, server));
        let (read, mut write) = client.into_split();
        let mut read = BufReader::new(read);
        let mut body = serde_json::to_vec(&Request {
            id: 1,
            method: method.into(),
            params,
        })
        .unwrap();
        body.push(b'\n');
        write.write_all(&body).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(8), read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        drop(write);
        drop(read);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str::<crewd_proto::Response>(&line)
            .unwrap()
            .into_result()
    }
    async fn configure(&self, url: &str) {
        self.rpc(
            SETTINGS_SET,
            serde_json::to_value(JudgmentSettingsSet {
                request_id: uuid::Uuid::new_v4().to_string(),
                expected_revision: None,
                credential_update: CredentialUpdate::Set {
                    value: "private-synthetic-secretary-key".into(),
                },
                config: Some(JudgmentConfig {
                    enabled: true,
                    provider: "typesafe".into(),
                    base_url: url.into(),
                    model: "synthetic-native".into(),
                    choice_min: 0.5,
                    wake_yes: 0.5,
                    similar_requirements: None,
                    similar_asks: Some(true),
                    credential_source: "file".into(),
                }),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    }
    async fn two_questions(&self) -> Vec<String> {
        let worker = self.worker.clone();
        self.state.collab(move|tx|{
            ["Should the checked report be published? OPENAI_API_KEY=synthetic-do-not-expose","Can we publish the checked report?"].iter().map(|text|asks::open(tx,&worker,asks::OpenAsk{question:(*text).into(),default:None,resume_note:Some("source work stays independent".into())}).map(|a|a.ask)).collect::<crewd_collab::Result<Vec<_>>>()
        }).await.unwrap()
    }
    async fn page(&self) -> Value {
        self.rpc(
            crewd_proto::admin_ask_groups::LIST,
            json!({"crewId":self.crew}),
        )
        .await
        .unwrap()
    }
    async fn stop_db(&self) {
        self.runtime.shutdown().await;
        let db = self.state.db.clone();
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
    }
    async fn reopen(&mut self) {
        self.stop_db().await;
        let data = self.dir.path().canonicalize().unwrap();
        self.state = Arc::new(AppState::new(
            crewd_store::Db::spawn(
                crewd_store::Store::open(&data.join(crewd_store::DB_FILE)).unwrap(),
            )
            .unwrap(),
        ));
        self.state.judgment_test_http.store(true, Ordering::Release);
        self.state.admission.allow(&self.captain);
        self.state.admission.allow(&self.worker);
        self.runtime =
            super::super::Runtime::start_with_admin_dispatch(&self.state, data, true, false)
                .await
                .unwrap()
                .unwrap();
    }
}

fn answer() -> Reply {
    Reply::ok(json!({"model":"synthetic-native","answers":{"same_answer_needed":{"type":"noul","noul":0.91}}}).to_string())
}

#[tokio::test]
async fn actual_periodic_native_suggestion_is_human_read_only_and_never_an_answer() {
    let fake = FakeJev::start(vec![answer()]).await;
    let f = Fixture::new().await;
    let ids = f.two_questions().await;
    assert_eq!(f.page().await["availability"], "disabled");
    assert!(fake.requests().is_empty());
    f.configure(&fake.base_url).await;
    let page = tokio::time::timeout(Duration::from_secs(18), async {
        loop {
            let p = f.page().await;
            if p["items"]
                .as_array()
                .is_some_and(|v| v.iter().any(|i| i["state"] == "suggested"))
            {
                break p;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("the real bounded periodic source scan and provider must settle");
    assert_eq!(page["items"][0]["first"]["id"], ids[0]);
    assert_eq!(page["items"][0]["second"]["id"], ids[1]);
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].body_text().contains("synthetic-do-not-expose"));
    assert!(
        !requests[0]
            .body_text()
            .contains("private-synthetic-secretary-key")
    );
    assert!(!requests[0].body_text().contains(&f.crew));
    assert!(!requests[0].body_text().contains("sourceMessageId"));
    let crew = f.crew.clone();
    f.state
        .db
        .call_result(move |s| {
            for id in ids {
                assert_eq!(
                    asks::get(s.conn(), &crew, &id)?.state,
                    crewd_proto::AskStateView::Open
                );
            }
            Ok::<_, crewd_collab::CollabError>(())
        })
        .await
        .unwrap()
        .unwrap();
    f.stop_db().await;
}

#[tokio::test]
async fn actual_db_reopen_preserves_dispatched_unknown_without_a_second_provider_call() {
    let fake = FakeJev::start(vec![answer().delayed(Duration::from_secs(1))]).await;
    let mut f = Fixture::new().await;
    f.two_questions().await;
    f.configure(&fake.base_url).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while fake.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.reopen().await;
    let page = f.page().await;
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["state"] == "unknown")
    );
    // Wait through another real scheduling interval with valid fresh admission.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(fake.requests().len(), 1);
    let after = f.page().await;
    assert!(
        after["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["state"] == "unknown")
    );
    assert_eq!(after["partialCoverage"], true);
    f.stop_db().await;
}

#[tokio::test]
async fn actual_settings_original_uuid_recovery_does_not_publish_a_second_invalidation() {
    let f = Fixture::new().await;
    let mut events = f.state.hub.subscribe();
    let request = JudgmentSettingsSet {
        request_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: None,
        config: None,
        credential_update: CredentialUpdate::Keep,
    };
    let packet = serde_json::to_value(&request).unwrap();
    let first = f.rpc(SETTINGS_SET, packet.clone()).await.unwrap();
    let mut count = 0;
    while let Ok(event) = events.try_recv() {
        if event.event == crewd_proto::admin_ask_groups::INVALIDATED {
            assert_eq!(event.data, json!({}));
            count += 1;
        }
    }
    assert_eq!(
        count, 1,
        "only the actual durable settings commit publishes"
    );
    let recovered = f.rpc(SETTINGS_SET, packet).await.unwrap();
    assert_eq!(recovered, first);
    while let Ok(event) = events.try_recv() {
        assert_ne!(
            event.event,
            crewd_proto::admin_ask_groups::INVALIDATED,
            "lookup of the original UUID cannot publish a fresh event"
        );
    }
    f.stop_db().await;
}
