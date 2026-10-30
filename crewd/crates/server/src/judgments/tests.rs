use super::*;
use crate::public_commands::Actor;
use crewd_collab::{Sender, crews, messages, requirements, sessions};
use crewd_proto::{Request, Role};
#[expect(
    clippy::duplicate_mod,
    reason = "Reuse the existing fake HTTP fixture in this test-only judgment namespace and the independent SQLite projection tests"
)]
#[path = "../../../jev/tests/support/mod.rs"]
mod support;
use support::{FakeJev, Reply};
struct Fixture {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    runtime: Arc<Runtime>,
    actor: Actor,
    crew: String,
    worker: String,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().canonicalize().unwrap();
        let state = Arc::new(AppState::new(
            crewd_store::Db::spawn(
                crewd_store::Store::open(&data.join(crewd_store::DB_FILE)).unwrap(),
            )
            .unwrap(),
        ));
        state.judgment_test_http.store(true, Ordering::Release);
        let crew = state
            .collab(|tx| {
                crews::create(
                    tx,
                    crews::NewCrew {
                        title: "fake private crew".into(),
                        working_directory: "/tmp/fake-owned-project".into(),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        let cid = crew.clone();
        let _cap = state
            .collab(move |tx| {
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: cid,
                        role: Role::Captain,
                        executor: "claude".into(),
                        title: Some("captain".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        let cid = crew.clone();
        let worker = state
            .collab(move |tx| {
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: cid,
                        role: Role::Worker,
                        executor: "claude".into(),
                        title: Some("worker".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        let policy = Arc::new(crate::peer::PeerPolicy::new(None).unwrap());
        let (socket, _client) = tokio::net::UnixStream::pair().unwrap();
        let actor = Actor::Local(policy.accept(&socket).unwrap());
        // These routing scenarios represent a team explicitly activated by Human
        // in this daemon run; reopen intentionally does not restore admission.
        state.admission.allow(&_cap);
        state.admission.allow(&worker);
        let runtime = Runtime::start(&state, data, true).await.unwrap().unwrap();
        Self {
            dir,
            state,
            runtime,
            actor,
            crew,
            worker,
        }
    }
    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let state = self.state.clone();
        let task = tokio::spawn(crate::conn::run(state, server));
        let (read, mut write) = client.into_split();
        let mut read = BufReader::new(read);
        let request = Request {
            id: 1,
            method: method.into(),
            params,
        };
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        write.write_all(&bytes).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(8), read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        write.shutdown().await.unwrap();
        drop(read);
        drop(write);
        tokio::time::timeout(std::time::Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str::<crewd_proto::Response>(&line)
            .unwrap()
            .into_result()
    }
    fn config(url: &str) -> JudgmentConfig {
        JudgmentConfig {
            enabled: true,
            provider: "typesafe".into(),
            base_url: url.into(),
            model: "fake-native".into(),
            choice_min: 0.5,
            wake_yes: 0.5,
            credential_source: "file".into(),
        }
    }
    async fn configure(&self, url: &str) -> JudgmentSettingsResult {
        serde_json::from_value(
            self.rpc(
                SETTINGS_SET,
                serde_json::to_value(JudgmentSettingsSet {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    expected_revision: None,
                    config: Some(Self::config(url)),
                    credential_update: CredentialUpdate::Set {
                        value: "private-fake-key".into(),
                    },
                })
                .unwrap(),
            )
            .await
            .unwrap(),
        )
        .unwrap()
    }
    async fn settled(&self, kind: JudgmentSourceKind, id: String) -> JudgmentRecord {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let k = kind.clone();
                let i = id.clone();
                let row = self
                    .state
                    .db
                    .call_result(move |s| facts::record(s.conn(), &k, &i))
                    .await
                    .unwrap()
                    .unwrap();
                if let Some(row) = row
                    && !matches!(
                        row.phase,
                        JudgmentPhase::Pending | JudgmentPhase::Dispatched
                    )
                {
                    break row;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
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
        self.runtime = Runtime::start(&self.state, data, true)
            .await
            .unwrap()
            .unwrap();
        let policy = Arc::new(crate::peer::PeerPolicy::new(None).unwrap());
        let (server, _client) = tokio::net::UnixStream::pair().unwrap();
        self.actor = Actor::Local(policy.accept(&server).unwrap());
    }
    async fn close(self) {
        self.stop_db().await;
    }
}
#[tokio::test]
async fn production_runtime_routes_wakes_attributes_and_recovery_does_not_redispatch() {
    let fake=FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"route":{"type":"choice","choice":"s1","confidence":0.9,"probabilities":{"s0":0.1,"s1":0.9}}},"usage":{"input_tokens":7,"output_tokens":2}}).to_string()),Reply::ok(json!({"model":"fake-native","answers":{"wake_s0":{"type":"noul","noul":0.1},"wake_s1":{"type":"noul","noul":0.9},"attribute":{"type":"choice","choice":"o0","confidence":0.8,"probabilities":{"o0":0.8,"none":0.2}}}}).to_string())]).await;
    let mut f = Fixture::new().await;
    // Prior source is real, unconfigured and never later enrolled by enabling Jev.
    let ch = format!("{}/lobby", f.crew);
    let old = f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: ch,
                    text: "prior context".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    let configured = f.configure(&fake.base_url).await;
    assert_eq!(configured.outcome, JudgmentConfigurationOutcome::Committed);
    assert_eq!(
        configured.view.credential_state,
        CredentialState::RecordedUnverified
    );
    let crew = f.crew.clone();
    let req = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "route this immutable requirement".into(),
                },
            )
        })
        .await
        .unwrap();
    let id = req.todo.requirement_id.unwrap();
    let route = f.settled(JudgmentSourceKind::Requirement, id.clone()).await;
    assert_eq!(route.phase, JudgmentPhase::Settled);
    assert_eq!(route.recipients, vec![f.worker.clone()]);
    let ch = format!("{}/lobby", f.crew);
    let m = f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: ch,
                    text: "this concerns the prior context".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    let attributed = f.settled(JudgmentSourceKind::Message, m.id.clone()).await;
    assert_eq!(attributed.recipients, vec![f.worker.clone()]);
    assert_eq!(attributed.attribution.as_ref().unwrap().id, old.id);
    let target = f.worker.clone();
    let rows = f
        .state
        .db
        .call_result(move |s| crewd_collab::wakes::list(s.conn(), Some(&target), 100))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.iter()
            .filter(|w| w.source == format!("judgment-route:{id}"))
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|w| w.source == crewd_collab::wakes::source::message(&m.id))
            .count(),
        1
    );
    assert_eq!(fake.requests().len(), 2);
    f.runtime
        .run(JudgmentSourceKind::Requirement, id, true)
        .await;
    f.runtime
        .run(JudgmentSourceKind::Message, m.id.clone(), true)
        .await;
    assert_eq!(fake.requests().len(), 2);
    let inspection = f
        .rpc(INSPECT, json!({"crewId":f.crew,"kind":"message","id":m.id}))
        .await
        .unwrap();
    f.reopen().await;
    let restored_inspection = f
        .rpc(INSPECT, json!({"crewId":f.crew,"kind":"message","id":m.id}))
        .await
        .unwrap();
    assert_eq!(inspection, restored_inspection);
    assert_eq!(fake.requests().len(), 2);
    let encoded = inspection.to_string();
    for forbidden in [
        "raw_usage",
        "capturedFloors",
        "dbId",
        "private-fake-key",
        "prior context",
    ] {
        assert!(!encoded.contains(forbidden), "{forbidden}")
    }
    f.close().await;
}
#[tokio::test]
async fn settings_unknown_barrier_is_not_retried_and_endpoint_keep_never_reuses_secret() {
    let mut f = Fixture::new().await;
    let first = f.configure("http://127.0.0.1:1/old").await;
    let revision = first.view.settings.revision.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let bad = JudgmentSettingsSet {
        request_id: id.clone(),
        expected_revision: revision.clone(),
        config: Some(Fixture::config("http://127.0.0.1:1/new")),
        credential_update: CredentialUpdate::Keep,
    };
    assert_eq!(
        f.rpc(SETTINGS_SET, serde_json::to_value(bad).unwrap())
            .await
            .unwrap_err()
            .code,
        crewd_proto::RpcError::INVALID_PARAMS
    );
    let current: JudgmentSettingsResponse =
        serde_json::from_value(f.rpc(SETTINGS_GET, json!({})).await.unwrap()).unwrap();
    assert_eq!(current.view.settings.revision, revision);
    assert!(current.view.pending_request_id.is_none());
    // A pre-existing unsafe private storage directory forces an actual controlled
    // file operation failure after durable intent, not a provider/Keychain call.
    let data = f.dir.path().canonicalize().unwrap();
    let keys = data.join("judgment-keys");
    let saved = data.join("saved-private-keys");
    std::fs::rename(&keys, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &keys).unwrap();
    let request = JudgmentSettingsSet {
        request_id: id.clone(),
        expected_revision: revision.clone(),
        config: Some(Fixture::config("http://127.0.0.1:1/new")),
        credential_update: CredentialUpdate::Set {
            value: "new-fake-only-key".into(),
        },
    };
    let unknown: JudgmentSettingsResult = serde_json::from_value(
        f.rpc(SETTINGS_SET, serde_json::to_value(&request).unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(unknown.outcome, JudgmentConfigurationOutcome::Unknown);
    assert_eq!(
        unknown.view.credential_state,
        CredentialState::UpdateUnknown
    );
    f.reopen().await;
    let looked: JudgmentSettingsResponse =
        serde_json::from_value(f.rpc(SETTINGS_GET, json!({"requestId":id})).await.unwrap())
            .unwrap();
    assert_eq!(
        looked.request_lookup,
        Some(JudgmentRequestLookup::Known {
            result: Box::new(unknown.clone())
        })
    );
    let absent = f
        .rpc(
            SETTINGS_GET,
            json!({"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(absent["requestLookup"]["status"], "not_found");
    std::fs::remove_file(&keys).unwrap();
    std::fs::rename(&saved, &keys).unwrap();
    let before = std::fs::read_dir(&keys).unwrap().count();
    let same = f
        .rpc(SETTINGS_SET, serde_json::to_value(&request).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_value::<JudgmentSettingsResult>(same).unwrap(),
        unknown
    );
    assert_eq!(std::fs::read_dir(&keys).unwrap().count(), before);
    let mut explicit = request;
    explicit.request_id = uuid::Uuid::new_v4().to_string();
    let recovered: JudgmentSettingsResult = serde_json::from_value(
        f.rpc(SETTINGS_SET, serde_json::to_value(explicit).unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.outcome, JudgmentConfigurationOutcome::Committed);
    assert!(recovered.view.pending_request_id.is_none());
    f.actor.connection_scope().close();
    assert_eq!(
        handle(
            &f.state,
            &f.actor,
            Request {
                id: 2,
                method: SETTINGS_GET.into(),
                params: json!({})
            }
        )
        .await
        .into_result()
        .unwrap_err()
        .code,
        crewd_proto::RpcError::FORBIDDEN
    );
    f.close().await;
}

#[tokio::test]
async fn restarted_dispatched_judgment_uses_captain_fallback_without_provider_replay() {
    let fake = FakeJev::start(vec![]).await;
    let mut f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    f.runtime.shutdown().await;
    let crew = f.crew.clone();
    let req = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "pending before restart".into(),
                },
            )
        })
        .await
        .unwrap();
    let id = req.todo.requirement_id.unwrap();
    let source = id.clone();
    f.state
        .collab(move |tx| facts::dispatch(tx, &JudgmentSourceKind::Requirement, &source))
        .await
        .unwrap()
        .unwrap();
    f.reopen().await;
    let state = f.state.clone();
    let runtime = f.runtime.clone();
    let source = id.clone();
    let row = state
        .db
        .call_result(move |s| facts::record(s.conn(), &JudgmentSourceKind::Requirement, &source))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(row.phase, JudgmentPhase::Settled);
    assert_eq!(row.diagnostic["reason"], "restart_unknown");
    assert_eq!(row.recipients.len(), 1);
    assert_eq!(fake.requests().len(), 0);
    runtime.run(JudgmentSourceKind::Requirement, id, true).await;
    assert_eq!(fake.requests().len(), 0);
    f.reopen().await;
    assert_eq!(fake.requests().len(), 0);
    f.close().await;
}
#[tokio::test]
async fn stop_during_actual_fake_request_suppresses_late_route_without_new_wakes() {
    let fake=FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"route":{"type":"choice","choice":"s1","confidence":0.9,"probabilities":{"s0":0.1,"s1":0.9}}}}).to_string()).delayed(std::time::Duration::from_millis(100))]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let crew = f.crew.clone();
    let req = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "Stop wins over a late judgment".into(),
                },
            )
        })
        .await
        .unwrap();
    let id = req.todo.requirement_id.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while fake.requests().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await
        }
    })
    .await
    .unwrap();
    let worker = f.worker.clone();
    f.state
        .collab(move |tx| {
            crewd_collab::wakes::human_stop(tx, &worker)?;
            sessions::set_desired(tx, &worker, crewd_proto::Desired::Running)
        })
        .await
        .unwrap();
    let row = f.settled(JudgmentSourceKind::Requirement, id.clone()).await;
    assert_eq!(row.phase, JudgmentPhase::Suppressed);
    let source = format!("judgment-route:{id}");
    let rows = f
        .state
        .db
        .call_result(move |s| {
            s.conn().query_row(
                "SELECT COUNT(*) FROM wakes WHERE source=?1",
                [source],
                |r| r.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(fake.requests().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn routed_requirement_reaches_production_db_actor_once_and_only_typed_ack_settles_delivery() {
    use crate::orchestra::{
        self,
        clock::SystemClock,
        db::DbPort,
        exec::Executors,
        hooks::Hooks,
        testkit::{StandIn, StandInWorktrees},
    };
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    let fake=FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"route":{"type":"choice","choice":"s1","confidence":0.9,"probabilities":{"s0":0.1,"s1":0.9}}}}).to_string())]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let rx = f.state.hub.subscribe_collab();
    let exec = StandIn::default();
    let hooks = Hooks {
        worktrees: Arc::new(StandInWorktrees::default()),
        ..Default::default()
    };
    let (handle, actor) = orchestra::spawn(
        Arc::new(DbPort::new(
            f.state.clone(),
            f.dir.path().join("unused-in-memory-db"),
        )),
        Executors::new().with("claude", Arc::new(exec.clone())),
        hooks,
        Arc::new(SystemClock),
        orchestra::OrchestraConfig::for_tests(),
    )
    .await
    .unwrap();
    let bridge = orchestra::db::forward_collab(rx, handle.clone());
    let crew = f.crew.clone();
    let req = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "exact Jev route actor input".into(),
                },
            )
        })
        .await
        .unwrap();
    let id = req.todo.requirement_id.unwrap();
    let marker = format!("judgment-route:{id}");
    f.settled(JudgmentSourceKind::Requirement, id.clone()).await;
    let mut brief_ack = false;
    let (delivery, subject) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            handle.latches().await.unwrap();
            if let Some(initial) = exec
                .launches(&f.worker)
                .first()
                .and_then(|l| l.initial.clone())
            {
                if initial.contains("newRequirement") {
                    break (BRIEF_DELIVERY_ID, Subject::Brief);
                }
                if !brief_ack {
                    exec.emit(
                        &f.worker,
                        SessionEvent::Delivery {
                            id: BRIEF_DELIVERY_ID,
                            subject: Subject::Brief,
                            outcome: DeliveryOutcome::Delivered,
                        },
                    );
                    brief_ack = true;
                }
            }
            if exec
                .sent(&f.worker)
                .iter()
                .any(|text| text.contains("newRequirement"))
            {
                break (exec.last_send_id(&f.worker), Subject::Steer);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let source = marker.clone();
    let before = f
        .state
        .db
        .call_result(move |s| {
            crewd_collab::wakes::list(s.conn(), None, 100)
                .map(|rows| rows.into_iter().find(|r| r.source == source).unwrap())
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.state, crewd_collab::wakes::WakeState::Inflight);
    exec.emit(
        &f.worker,
        SessionEvent::Delivery {
            id: delivery,
            subject,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            handle.latches().await.unwrap();
            let source = marker.clone();
            let delivered = f
                .state
                .db
                .call_result(move |s| {
                    crewd_collab::wakes::list(s.conn(), None, 100).map(|rows| {
                        rows.into_iter().any(|r| {
                            r.source == source
                                && r.state == crewd_collab::wakes::WakeState::Delivered
                        })
                    })
                })
                .await
                .unwrap()
                .unwrap();
            if delivered {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.runtime
        .run(JudgmentSourceKind::Requirement, id, true)
        .await;
    handle.latches().await.unwrap();
    assert_eq!(fake.requests().len(), 1);
    let inputs = exec
        .sent(&f.worker)
        .iter()
        .filter(|text| text.contains("newRequirement"))
        .count()
        + exec
            .launches(&f.worker)
            .iter()
            .filter(|l| {
                l.initial
                    .as_ref()
                    .is_some_and(|s| s.contains("newRequirement"))
            })
            .count();
    assert_eq!(inputs, 1);
    f.runtime.shutdown().await;
    assert!(handle.shutdown(std::time::Duration::from_secs(2)).await);
    actor.finished().await.unwrap();
    bridge.abort();
    let _ = bridge.await;
    f.close().await;
}

#[tokio::test]
async fn official_restore_same_db_id_histories_judgments_and_request_without_loading_old_secret() {
    let fake = FakeJev::start(vec![]).await;
    let mut f = Fixture::new().await;
    let configured = f.configure(&fake.base_url).await;
    f.runtime.shutdown().await;
    let crew = f.crew.clone();
    let req = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "old admitted intent".into(),
                },
            )
        })
        .await
        .unwrap();
    let id = req.todo.requirement_id.unwrap();
    let source = id.clone();
    f.state
        .collab(move |tx| facts::dispatch(tx, &JudgmentSourceKind::Requirement, &source))
        .await
        .unwrap()
        .unwrap();
    let before = f
        .state
        .db
        .call_result(|s| {
            Ok::<_, crewd_store::StoreError>((
                crewd_store::meta_get(s.conn(), "dbId")?.unwrap(),
                crewd_store::judgment_state::epoch(s.conn())?,
            ))
        })
        .await
        .unwrap()
        .unwrap();
    f.stop_db().await;
    let data = f.dir.path().canonicalize().unwrap();
    let backup = data.join("original-intent-backup.db");
    let lock = crate::try_lock_data_dir(&data).unwrap().unwrap();
    crewd_store::backup::backup_db_quiescent(&data.join(crewd_store::DB_FILE), &backup).unwrap();
    crewd_store::backup::restore(&data, &backup).unwrap();
    drop(lock);
    f.reopen().await;
    let after = f
        .state
        .db
        .call_result(|s| {
            Ok::<_, crewd_store::StoreError>((
                crewd_store::meta_get(s.conn(), "dbId")?.unwrap(),
                crewd_store::judgment_state::epoch(s.conn())?,
            ))
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.0, after.0);
    assert_ne!(before.1, after.1);
    let inspect = f
        .rpc(
            INSPECT,
            json!({"crewId":f.crew,"kind":"requirement","id":id}),
        )
        .await
        .unwrap();
    assert_eq!(inspect["record"]["phase"], "historical");
    let query = f
        .rpc(SETTINGS_GET, json!({"requestId":configured.request_id}))
        .await
        .unwrap();
    assert_eq!(query["requestLookup"]["status"], "historical");
    assert!(query["requestLookup"].get("result").is_none());
    assert_eq!(query["view"]["credentialState"], "unknown");
    let crew = f.crew.clone();
    let fresh = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "new explicit Human fact after restore".into(),
                },
            )
        })
        .await
        .unwrap()
        .todo
        .requirement_id
        .unwrap();
    let settled = f.settled(JudgmentSourceKind::Requirement, fresh).await;
    assert_eq!(settled.diagnostic["reason"], "awaiting_manual_resume");
    assert_eq!(fake.requests().len(), 0);
    f.close().await;
}

#[tokio::test]
async fn credential_never_enters_generic_rpc_result_or_sqlite_business_storage() {
    let f = Fixture::new().await;
    let result = f.configure("http://127.0.0.1:1").await;
    let sentinel = b"private-fake-key";
    assert!(
        !serde_json::to_vec(&result)
            .unwrap()
            .windows(sentinel.len())
            .any(|w| w == sentinel)
    );
    let view = f
        .rpc("judgment.settings.get", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(
        !serde_json::to_vec(&view)
            .unwrap()
            .windows(sentinel.len())
            .any(|w| w == sentinel)
    );
    f.stop_db().await;
    // SQLite's closed on-disk image, including any sidecar that remains, covers
    // generic receipt/meta/event storage without exempting an individual table.
    for name in [
        crewd_store::DB_FILE.to_string(),
        format!("{}-wal", crewd_store::DB_FILE),
    ] {
        let path = f.dir.path().join(name);
        if path.exists() {
            let bytes = std::fs::read(path).unwrap();
            assert!(!bytes.windows(sentinel.len()).any(|w| w == sentinel));
        }
    }
}

#[tokio::test]
async fn old_core_restore_witness_rejects_secret_and_cannot_be_laundered_by_configure() {
    let fake = FakeJev::start(vec![]).await;
    let mut f = Fixture::new().await;
    let configured = f.configure(&fake.base_url).await;
    let settings = configured.view.settings.clone();
    let before = f.runtime.current_provider(&settings).await.unwrap();
    assert!(
        configuration::load_key(&f.state, f.runtime.data.clone(), &settings)
            .await
            .is_some()
    );
    f.state
        .collab(|tx| {
            let revision = crewd_store::remote::configuration(tx.conn())?
                .revision
                .parse()
                .unwrap();
            assert!(crewd_store::remote::configure(
                tx.conn(),
                revision,
                false,
                "127.0.0.1",
                7424
            )?);
            Ok(())
        })
        .await
        .unwrap();
    let after = f.runtime.current_provider(&settings).await.unwrap();
    assert!(Arc::ptr_eq(&before, &after));
    assert!(
        configuration::load_key(&f.state, f.runtime.data.clone(), &settings)
            .await
            .is_some()
    );
    assert!(fake.requests().is_empty());
    f.state
        .collab(|tx| {
            // Exactly the old14 formal replacement hook, with no new Jev hook.
            crewd_store::remote::invalidate_for_work_replacement(
                tx.conn(),
                "work_restore",
                "fake",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        configuration::load_key(&f.state, f.runtime.data.clone(), &settings)
            .await
            .is_none()
    );
    let stale = f.runtime.current_provider(&settings).await.unwrap();
    assert!(!Arc::ptr_eq(&before, &stale));
    f.state
        .collab(|tx| {
            let revision = crewd_store::remote::configuration(tx.conn())?
                .revision
                .parse()
                .unwrap();
            assert!(crewd_store::remote::configure(
                tx.conn(),
                revision,
                false,
                "127.0.0.1",
                7425
            )?);
            Ok(())
        })
        .await
        .unwrap();
    f.reopen().await;
    assert!(
        configuration::load_key(&f.state, f.runtime.data.clone(), &settings)
            .await
            .is_none()
    );
    let query = f
        .rpc(SETTINGS_GET, json!({"requestId":configured.request_id}))
        .await
        .unwrap();
    assert_eq!(query["requestLookup"]["status"], "historical");
    assert_eq!(query["view"]["credentialState"], "unknown");
    assert!(fake.requests().is_empty());
    f.close().await;
}

#[tokio::test]
async fn sqlite_jev_never_calls_provider_before_manual_current_run_admission() {
    let fake = FakeJev::start(vec![]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    f.state.admission.close();
    let crew = f.crew.clone();
    let req = f
        .state
        .collab(move |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    crew_id: crew,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: None,
                    text: "visible data does not authorize execution".into(),
                },
            )
        })
        .await
        .unwrap();
    let row = f
        .settled(
            JudgmentSourceKind::Requirement,
            req.todo.requirement_id.unwrap(),
        )
        .await;
    assert_eq!(row.diagnostic["reason"], "awaiting_manual_resume");
    assert!(fake.requests().is_empty());
    f.close().await;
}

#[tokio::test]
async fn fresh_human_message_admits_only_jev_chosen_new_target() {
    let fake = FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"wake_s0":{"type":"noul","noul":0.1},"wake_s1":{"type":"noul","noul":0.9}}}).to_string())]).await;
    let f = Fixture::new().await;
    let ids = f
        .state
        .db
        .call_result(|s| sessions::list(s.conn(), None))
        .await
        .unwrap()
        .unwrap();
    for s in ids {
        f.state.admission.revoke(&s.id);
    }
    assert!(!f.state.admission.any());
    f.configure(&fake.base_url).await;
    let channel = format!("{}/lobby", f.crew);
    let message = f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: channel,
                    text: "new explicit Human message from quiet startup".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    let record = f
        .settled(JudgmentSourceKind::Message, message.id.clone())
        .await;
    assert_eq!(
        fake.requests().len(),
        1,
        "new Human intent must not get stuck behind empty admission"
    );
    assert_eq!(record.recipients, vec![f.worker.clone()]);
    // Settlement is durable before the post-commit admission read completes.
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    while !f.state.admission.contains(&f.worker) {
        assert!(
            tokio::time::Instant::now() < until,
            "chosen current target was not admitted after commit"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        f.state
            .db
            .call_result(|s| sessions::list(s.conn(), None))
            .await
            .unwrap()
            .unwrap()
            .iter()
            .filter(|s| f.state.admission.contains(&s.id))
            .count(),
        1,
        "only selected actual target may be admitted"
    );
    f.state.admission.revoke(&f.worker);
    f.runtime
        .run(JudgmentSourceKind::Message, message.id.clone(), false)
        .await;
    f.runtime
        .run(JudgmentSourceKind::Message, message.id, true)
        .await;
    assert!(
        !f.state.admission.any(),
        "duplicate/recovery must not revive old intent"
    );
    assert_eq!(fake.requests().len(), 1);
    // Simulate a persisted Pending fact whose live event was missed. Recovery
    // settles the original record but never treats that scan as a new send.
    let channel = format!("{}/lobby", f.crew);
    let missed = f
        .state
        .db
        .call_result(move |s| {
            crewd_collab::run(s, crewd_store::now_ms(), |tx| {
                messages::post(
                    tx,
                    Sender::Human,
                    messages::NewPost {
                        channel_id: channel,
                        text: "old pending fact is not a new live event".into(),
                        ..Default::default()
                    },
                )
            })
        })
        .await
        .unwrap()
        .unwrap()
        .0
        .message;
    f.runtime.scan_pending(true).await.unwrap();
    f.settled(JudgmentSourceKind::Message, missed.id).await;
    assert_eq!(fake.requests().len(), 1);
    assert!(
        !f.state.admission.any(),
        "recovery does not admit old pending work"
    );
    f.state.admission.close();
    let channel = format!("{}/lobby", f.crew);
    let closed = f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: channel,
                    text: "closed admission is not new permission".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    f.settled(JudgmentSourceKind::Message, closed.id).await;
    assert_eq!(
        fake.requests().len(),
        1,
        "closed admission must not call the fake provider"
    );
    assert!(!f.state.admission.any());
    f.close().await;
}
