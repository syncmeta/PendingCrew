//! Independent execution acceptance using real case sources, local native provider and Core.
#[path = "admin_activation_review.rs"]
mod activation_review;
#[path = "admin_legacy_review.rs"]
mod legacy_review;
use super::*;
use crate::orchestra::{
    self,
    clock::SystemClock,
    db::DbPort,
    exec::Executors,
    hooks::Hooks,
    testkit::{StandIn, StandInWorktrees},
};
use crewd_collab::{
    admin_case_execution as execution, admin_cases as cases, stop_checks, wakes, work,
};
use crewd_proto::{AdminCaseGetParams, AdminCaseListParams, Desired, WorkCreateParams};
use std::time::Duration;
fn answer(p: f64) -> Reply {
    Reply::ok(
        json!({"model":"fake-native","answers":{"needs_intervention":{"type":"noul","noul":p}}})
            .to_string(),
    )
}
#[derive(Clone)]
struct Source {
    id: String,
    work: String,
    owner: String,
}
async fn sources(f: &Fixture, n: usize) -> Vec<Source> {
    source_fixture(f, n, false).await
}
async fn source_fixture(f: &Fixture, n: usize, include_foreign: bool) -> Vec<Source> {
    source_fixture_with_link(f, n, include_foreign, false).await
}
async fn source_fixture_with_link(f: &Fixture, n: usize, include_foreign: bool, link: bool) -> Vec<Source> {
    let crew = f.crew.clone();
    let worker = f.worker.clone();
    let (made, _changes) = f
        .state
        .db
        .call_result(move |store| {
            if crews::management(store.conn())?.is_none() {
                crewd_store::crews::upsert(
                    store.conn(),
                    &crewd_proto::Crew {
                        id: "admin-test-management".into(),
                        title: "synthetic management".into(),
                        management: true,
                        created_at: "2026-09-01T00:00:00.000Z".into(),
                        updated_at: "2026-09-01T00:00:00.000Z".into(),
                        ..Default::default()
                    },
                    None,
                )?;
                crewd_store::schema::backfill_v3(store.conn())?;
                crewd_store::schema::backfill_v4(store.conn())?;
            }
            crewd_collab::run(store, crewd_store::now_ms(), move |tx| {
                let mut out = vec![];
                for i in 0..n {
                    let owner = if i == 0 {
                        worker.clone()
                    } else {
                        sessions::register(
                            tx,
                            sessions::NewSession {
                                crew_id: crew.clone(),
                                role: Role::Worker,
                                executor: "claude".into(),
                                title: Some(format!("case source {i}")),
                                ..Default::default()
                            },
                        )?
                        .id
                    };
                    sessions::set_desired(tx, &owner, Desired::Running)?;
                    let work = work::create(
                        tx,
                        &Sender::Human,
                        WorkCreateParams {
                            crew_id: crew.clone(),
                            owner: Some(owner.clone()),
                            title: format!("administrative source {i}"),
                            ..Default::default()
                        },
                    )?;
                    if link {
                        work::create(tx, &Sender::Human, WorkCreateParams {
                            crew_id: crew.clone(), owner: Some(owner.clone()), title: "linked administrative source".into(),
                            parent: Some(work.id.clone()), ..Default::default()
                        })?;
                    }
                    if include_foreign {
                        let foreign = crews::create(
                            tx,
                            crews::NewCrew {
                                title: "foreign source crew".into(),
                                working_directory: "/tmp/synthetic-foreign-private".into(),
                                ..Default::default()
                            },
                        )?;
                        work::create(
                            tx,
                            &Sender::Human,
                            WorkCreateParams {
                                crew_id: foreign.id,
                                owner: Some(owner.clone()),
                                title: "FOREIGN_PRIVATE_ADMIN_SOURCE_SENTINEL".into(),
                                ..Default::default()
                            },
                        )?;
                    }
                    for _ in 0..3 {
                        let c = stop_checks::candidate(tx.conn(), &owner, tx.now_ms())?
                            .expect("real stop-check candidate");
                        let stop_checks::Accepted::Rewake(w) = stop_checks::accept(tx, &c)? else {
                            panic!("expected original reminder")
                        };
                        wakes::claim(tx, &[w])?;
                        wakes::ack_delivered(tx, &owner, &[w], wakes::Riders::Release, &[], &[])?;
                    }
                    let c = stop_checks::candidate(tx.conn(), &owner, tx.now_ms())?.unwrap();
                    assert_eq!(
                        stop_checks::accept(tx, &c)?,
                        stop_checks::Accepted::Escalated
                    );
                    let page = cases::list(
                        tx.conn(),
                        &AdminCaseListParams {
                            crew_id: crew.clone(),
                            after: None,
                            limit: Some(100),
                        },
                    )?;
                    let id = page
                        .items
                        .into_iter()
                        .find(|c| c.source.session_id == owner)
                        .unwrap()
                        .id;
                    out.push(Source {
                        id,
                        work: work.id,
                        owner,
                    });
                }
                Ok(out)
            })
        })
        .await
        .unwrap()
        .unwrap();
    made
}
async fn view(f: &Fixture, id: &str) -> serde_json::Value {
    let crew = f.crew.clone();
    let id = id.to_owned();
    f.state
        .db
        .call_result(move |s| {
            Ok::<_, crewd_collab::CollabError>(serde_json::to_value(cases::get(
                s.conn(),
                &AdminCaseGetParams { crew_id: crew, id },
            )?)?)
        })
        .await
        .unwrap()
        .unwrap()
}

async fn acknowledge_initial_input(r: &Rig, session: &str) {
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    r.handle.latches().await.unwrap();
    let launch = r
        .exec
        .launches(session)
        .first()
        .cloned()
        .expect("actual runner launch");
    let delivery = if launch.initial.is_some() {
        Some((BRIEF_DELIVERY_ID, Subject::Brief))
    } else if launch.native_policy == orchestra::exec::NativeExecutionPolicy::AdminCaseRestricted {
        let id = r
            .exec
            .lock()
            .sends
            .get(session)
            .and_then(|s| s.first())
            .map(|(id, _)| *id)
            .expect("deferred case first input was actually queued");
        Some((id, Subject::Steer))
    } else {
        None
    };
    if let Some((id, subject)) = delivery {
        r.exec.emit(
            session,
            SessionEvent::Delivery {
                id,
                subject,
                outcome: DeliveryOutcome::Delivered,
            },
        );
    }
}

struct Rig {
    f: Fixture,
    handle: orchestra::Handle,
    _actor: orchestra::Actor,
    bridge: tokio::task::JoinHandle<()>,
    exec: StandIn,
}
impl Rig {
    async fn new(f: Fixture) -> Self {
        let exec = StandIn::default();
        let rx = f.state.hub.subscribe_collab();
        let mut cfg = orchestra::OrchestraConfig::for_tests();
        cfg.admission = Some(f.state.admission.clone());
        let (handle, actor) = orchestra::spawn(
            Arc::new(DbPort::new(
                f.state.clone(),
                f.dir.path().join(crewd_store::DB_FILE),
            )),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks {
                worktrees: Arc::new(StandInWorktrees::default()),
                ..Default::default()
            },
            Arc::new(SystemClock),
            cfg,
        )
        .await
        .unwrap();
        assert!(f.state.orchestra.set(handle.clone()).is_ok());
        let bridge = orchestra::db::forward_collab(rx, handle.clone());
        Self {
            f,
            handle,
            _actor: actor,
            bridge,
            exec,
        }
    }
    async fn close(self) {
        self.f.runtime.shutdown().await;
        self.exec.lock().hang_on_stop.clear();
        assert!(self.handle.shutdown(Duration::from_secs(2)).await);
        self.bridge.abort();
        self.f.close().await;
    }
}
#[tokio::test]
async fn native_adopted_case_produces_real_opaque_launch_authority() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    assert_eq!(
        view(&f, &source.id).await["decisionRecord"]["adopted"],
        true
    );
    let crew = f.crew.clone();
    let id = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &id))
        .await
        .unwrap();
    assert!(
        permit.is_some(),
        "native current adopted case must yield launch authority"
    );
    assert_eq!(fake.requests().len(), 1);
    f.close().await;
}

async fn prepared() -> (Fixture, Source, execution::AdminLaunchPermit, FakeJev) {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let id = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &id))
        .await
        .unwrap()
        .expect("adopted source permit");
    (f, source, permit, fake)
}

#[tokio::test]
async fn concurrent_case_launch_is_unique_and_ordinary_agent_cannot_create_admin() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let (a, b) = tokio::join!(
        r.handle.start_admin_case(permit.clone()),
        r.handle.start_admin_case(permit)
    );
    assert!(a.is_ok() || b.is_ok(), "a={a:?}; b={b:?}");
    let sid = a.or(b).unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.exec.launches(&sid).is_empty() {
            r.handle.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let admin = sid.clone();
    let row =
        r.f.state
            .db
            .call_result(move |s| sessions::get(s.conn(), &admin))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(row.role, Role::Admin);
    assert_eq!(row.crew_id, r.f.crew);
    assert_eq!(r.exec.launches(&sid).len(), 1);
    let count =
        r.f.state
            .db
            .call_result(|s| {
                s.conn().query_row(
                    "SELECT count(*) FROM sessions WHERE role='admin'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await
            .unwrap()
            .unwrap();
    assert_eq!(count, 1);
    let denied = r
        .handle
        .start(
            orchestra::core::StartSession {
                crew_id: r.f.crew.clone(),
                role: Role::Admin,
                executor: "claude".into(),
                brief: "unauthorized".into(),
                title: None,
                model: None,
                effort: None,
                isolation: false,
                codex_options: None,
            },
            orchestra::core::Origin::Agent,
        )
        .await;
    assert!(
        denied.is_err(),
        "ordinary Agent creation cannot borrow typed permit"
    );
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["sessionId"],
        sid
    );
    r.close().await;
}

#[tokio::test]
async fn source_admission_revoked_after_permit_causes_zero_launch_and_registration() {
    let (f, source, permit, _fake) = prepared().await;
    assert_eq!(permit.source_session(), source.owner);
    let owner = permit.source_session().to_owned();
    let r = Rig::new(f).await;
    r.f.state.admission.revoke(&owner);
    assert!(r.handle.start_admin_case(permit).await.is_err());
    r.handle.latches().await.unwrap();
    let count =
        r.f.state
            .db
            .call_result(|s| {
                s.conn().query_row(
                    "SELECT count(*) FROM sessions WHERE role='admin'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await
            .unwrap()
            .unwrap();
    assert_eq!(count, 0);
    r.close().await;
}

struct Socket {
    rd: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    wr: tokio::net::unix::OwnedWriteHalf,
    id: u64,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl Socket {
    fn owned(state: Arc<AppState>) -> Self {
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let task = tokio::spawn(crate::conn::run(state, server));
        let (rd, wr) = client.into_split();
        Self {
            rd: tokio::io::BufReader::new(rd),
            wr,
            id: 0,
            task,
        }
    }
    async fn attach(state: Arc<AppState>, sid: &str) -> Self {
        let token = state.tokens.mint(sid);
        let mut s = Self::owned(state);
        s.call("agent.attach", json!({"sessionId":sid,"token":token}))
            .await
            .unwrap();
        s
    }
    async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        self.id += 1;
        let mut bytes = serde_json::to_vec(&Request {
            id: self.id,
            method: method.into(),
            params,
        })
        .unwrap();
        bytes.push(b'\n');
        self.wr.write_all(&bytes).await.unwrap();
        loop {
            let mut line = String::new();
            assert!(
                tokio::time::timeout(Duration::from_secs(5), self.rd.read_line(&mut line))
                    .await
                    .unwrap()
                    .unwrap()
                    > 0
            );
            if let crewd_proto::ServerFrame::Response(r) = serde_json::from_str(&line).unwrap() {
                if r.id == self.id {
                    return r.into_result();
                }
            }
        }
    }
    async fn close(mut self) {
        use tokio::io::AsyncWriteExt;
        self.wr.shutdown().await.unwrap();
        drop(self.rd);
        drop(self.wr);
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn attached_case_cannot_use_other_work_raw_human_team_data_or_direct_tools() {
    let (f, source, permit, _fake) = prepared().await;
    let crew = f.crew.clone();
    let foreign = f
        .state
        .collab(move |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew,
                    title: "outside the case".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    for (method, args) in [
        ("agent.call", json!({"tool":"data_workspace_get","args":{}})),
        (
            "agent.call",
            json!({"tool":"post","args":{"text":"forbidden","channel":"lobby"}}),
        ),
        (
            "agent.call",
            json!({"tool":"work_item_get","args":{"id":foreign.id}}),
        ),
        (
            "channel.create",
            json!({"crewId":r.f.crew,"name":"forbidden"}),
        ),
        ("work.get", json!({"id":foreign.id})),
    ] {
        let result = socket.call(method, args).await;
        assert!(
            result.is_err() || result.as_ref().is_ok_and(|v| v["isError"] == true),
            "{method}: {result:?}"
        );
    }
    let me = crate::agent::port::identity(&r.f.state, &sid)
        .await
        .unwrap()
        .unwrap();
    let direct =
        crate::agent::tools::call(&r.f.state, &me, "work_item_get", json!({"id":source.work}))
            .await;
    assert!(
        direct.is_error,
        "direct call lacks authenticated case context"
    );
    socket.close().await;
    r.close().await;
}

async fn current_work(f: &Fixture, id: &str) -> crewd_proto::WorkPlan {
    let id = id.to_owned();
    f.state
        .db
        .call_result(move |s| work::get(s.conn(), &id))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn bound_native_catalog_unlink_has_exact_receipt_and_does_not_resolve_case() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = source_fixture_with_link(&f, 1, false, true).await.remove(0);
    f.runtime.run_admin_case(f.crew.clone(), source.id.clone(), true).await.unwrap();
    let original = view(&f, &source.id).await;
    let child = original["targets"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str())
        .find(|id| *id != source.work).unwrap();
    let child = current_work(&f, child).await;
    assert_eq!(child.parent.as_deref(), Some(source.work.as_str()));
    let parent = current_work(&f, &source.work).await;
    let crew = f.crew.clone();
    let id = source.id.clone();
    let (permit, outside) = f.state.collab(move |tx| {
        let outside = work::create(tx, &Sender::Human, WorkCreateParams {
            crew_id: crew.clone(), title: "same crew outside the case".into(), ..Default::default()
        })?;
        Ok((execution::launch_permit(tx, &crew, &id)?.unwrap(), outside))
    }).await.unwrap();
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    assert_eq!(r.exec.launches(&sid)[0].native_policy, orchestra::exec::NativeExecutionPolicy::AdminCaseRestricted);
    let mut admin = Socket::attach(r.f.state.clone(), &sid).await;
    let mut worker = Socket::attach(r.f.state.clone(), &r.f.worker).await;
    let tool = crewd_collab::admin_case_link::UNLINK_TOOL;
    let bound_tools = admin.call("agent.tools", json!({})).await.unwrap();
    assert!(bound_tools.as_array().unwrap().iter().any(|t| t["name"] == tool));
    let ordinary = worker.call("agent.tools", json!({})).await.unwrap();
    assert!(!ordinary.as_array().unwrap().iter().any(|t| t["name"] == tool));
    let params = json!({"tool":tool,"args":{"request_id":uuid::Uuid::new_v4().to_string(),"id":child.id,
        "expected_revision":child.revision,"related_id":parent.id,"expected_related_revision":parent.revision,
        "edge":"parent","evidence":{"kind":"text","value":"fixture evidence"},"note":"incorrect parent"}});
    let denied = worker.call("agent.call", params.clone()).await;
    assert!(denied.is_err() || denied.unwrap()["isError"] == true);
    let mut bad = params.clone();
    bad["args"]["related_id"] = json!(outside.id);
    bad["args"]["expected_related_revision"] = json!(outside.revision);
    let denied = admin.call("agent.call", bad).await;
    assert!(denied.is_err() || denied.unwrap()["isError"] == true);
    assert_eq!(current_work(&r.f, &child.id).await, child);

    let removed = admin.call("agent.call", params.clone()).await.unwrap();
    assert_eq!(removed["isError"], false, "{removed}");
    assert_eq!(admin.call("agent.call", params.clone()).await.unwrap(), removed);
    let after = current_work(&r.f, &child.id).await;
    assert!(after.parent.is_none());
    assert_eq!(after.status, child.status);
    assert_eq!(current_work(&r.f, &parent.id).await, parent);
    assert_eq!(view(&r.f, &source.id).await["executionRecord"]["effectsCount"], 1);
    let mut human = Socket::owned(r.f.state.clone());
    let report_params = json!({"crewId":r.f.crew,"id":source.id,"limit":1});
    let before = r.f.state.db.call(|s| s.conn().total_changes()).await.unwrap();
    let report = human.call(crewd_proto::admin_cases::EFFECTS, report_params.clone()).await.unwrap();
    assert_eq!(report["items"].as_array().unwrap().len(), 1);
    assert_eq!(report["items"][0]["kind"], "unlinked");
    assert_eq!(report["items"][0]["workId"], child.id);
    assert_eq!(report["items"][0]["relatedWorkId"], parent.id);
    assert_eq!(report["items"][0]["requestId"], params["args"]["request_id"]);
    assert_eq!(report["items"][0]["evidence"], params["args"]["evidence"]);
    assert_eq!(report["items"][0]["resultRevision"], after.revision);
    assert!(report["nextAfter"].is_null());
    assert_eq!(r.f.state.db.call(|s| s.conn().total_changes()).await.unwrap(), before);
    assert_eq!(
        admin.call(crewd_proto::admin_cases::EFFECTS, report_params).await.unwrap_err().code,
        crewd_proto::RpcError::FORBIDDEN
    );
    human.close().await;
    let crew = r.f.crew.clone();
    let case = source.id.clone();
    let outcome = r.f.state.collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &case)).await.unwrap();
    assert_ne!(outcome.phase, crewd_proto::AdminCaseOutcomePhase::Resolved);
    assert_eq!(fake.requests().len(), 1, "correction must not dispatch a model");
    r.f.state.admission.revoke(&source.owner);
    let denied = admin.call("agent.call", params).await;
    assert!(denied.is_err() || denied.unwrap()["isError"] == true);
    admin.close().await;
    worker.close().await;
    r.close().await;
}

fn close_args(work: &crewd_proto::WorkPlan) -> serde_json::Value {
    json!({"tool":"work_item_close","args":{"id":work.id,"expected_revision":work.revision,"kind":"done","evidence":{"kind":"text","value":"independent fixture verified completion"},"note":"actual administrative resolution"}})
}
#[tokio::test]
async fn legal_close_records_actual_work_actor_and_atomic_effect_checkpoint() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    let before = current_work(&r.f, &source.work).await;
    let result = socket
        .call("agent.call", close_args(&before))
        .await
        .unwrap();
    assert_eq!(result["isError"], false, "{result}");
    let after = current_work(&r.f, &source.work).await;
    assert_ne!(after.revision, before.revision);
    assert_eq!(serde_json::to_value(&after).unwrap()["status"], "done");
    let v = view(&r.f, &source.id).await;
    assert_eq!(v["executionRecord"]["effectsCount"], 1, "{v}");
    let events =
        r.f.state
            .db
            .call_result(|s| {
                let mut q = s
                    .conn()
                    .prepare("SELECT body FROM organization_work_events ORDER BY seq")?;
                q.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .unwrap()
            .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.contains(&sid) && e.contains(&source.work)),
        "no actual Work event attributed to Admin: {events:?}"
    );
    let again = socket.call("agent.call", close_args(&before)).await;
    assert!(again.is_err() || again.as_ref().is_ok_and(|v| v["isError"] == true));
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["effectsCount"],
        1
    );
    socket.close().await;
    r.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_revoked_while_real_write_queued_has_no_work_or_checkpoint_effect() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    let before = current_work(&r.f, &source.work).await;
    let args = close_args(&before);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *r.f.state.test_db_write_barrier.lock().unwrap() = Some(crate::DbWriteBarrier {
        entered: entered_tx,
        release: release_rx,
    });
    let pending = tokio::spawn(async move {
        let result = socket.call("agent.call", args).await;
        (socket, result)
    });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    r.f.state.tokens.revoke(&sid);
    release_tx.send(()).unwrap();
    let (socket, result) = pending.await.unwrap();
    assert!(
        result.is_err() || result.as_ref().is_ok_and(|v| v["isError"] == true),
        "{result:?}"
    );
    assert_eq!(
        current_work(&r.f, &source.work).await.revision,
        before.revision
    );
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["effectsCount"],
        0
    );
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn actual_started_case_file_database_reopen_never_creates_second_executor() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            r.handle.latches().await.unwrap();
            if view(&r.f, &source.id).await["executionRecord"]["phase"] == "running" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(r.exec.launches(&sid).len(), 1);
    r.f.runtime.shutdown().await;
    assert!(r.handle.shutdown(Duration::from_secs(2)).await);
    r.bridge.abort();
    let Rig {
        mut f,
        handle,
        _actor,
        bridge,
        exec,
    } = r;
    drop((handle, _actor, bridge, exec));
    f.reopen().await;
    assert_eq!(
        view(&f, &source.id).await["executionRecord"]["sessionId"],
        sid
    );
    assert_eq!(
        view(&f, &source.id).await["executionRecord"]["phase"],
        "unknown"
    );
    // Current-run Human source admission does not grant permission to replay an unknown launch.
    f.state.admission.allow(&source.owner);
    let crew = f.crew.clone();
    let id = source.id.clone();
    assert!(
        f.state
            .collab(move |tx| execution::launch_permit(tx, &crew, &id))
            .await
            .unwrap()
            .is_none()
    );
    let count = f
        .state
        .db
        .call_result(|s| {
            s.conn().query_row(
                "SELECT count(*) FROM sessions WHERE role='admin'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}

#[tokio::test]
async fn active_case_obligation_blocks_retirement_of_otherwise_idle_resumable_admin() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    r.handle.latches().await.unwrap();
    let admin = sid.clone();
    let crew = r.f.crew.clone();
    r.f.state.db.call_result(move |s| {
        // Set the original retained runner identity as the real adapters do; no case record is edited.
        crewd_store::sessions::set_agent_session_id(s.conn(),&admin,"retained-admin-fixture")?;
        let ((),_)=crewd_collab::run(s,crewd_store::now_ms(),|tx| {
            let row=sessions::get(tx.conn(),&admin)?;
            assert_eq!(row.desired,Desired::Running);
            assert!(!row.was_working);
            assert!(row.resume_failed_at.is_none());
            assert!(row.awaiting_question.is_none());
            let n:i64=tx.conn().query_row("SELECT count(*) FROM wakes WHERE session_id=?1 AND state IN ('pending','inflight')",[&admin],|r|r.get(0))?;
            assert_eq!(n,0);
            let book=crewd_store::work_organization::load(tx.conn())?.expect("organization work");
            assert!(!book.items.values().any(|w|w.owner.as_ref().is_some_and(|o|o.as_str()==admin)));
            assert!(!book.asks.values().any(|a|a.asked_by.session().is_some_and(|s|s.as_str()==admin)));
            let witness=crewd_collab::session_retirement::ActorWitness{incarnation:"test-physical-admin-generation".into(),generation:1,activity:0};
            assert!(crewd_collab::session_retirement::prepare(tx,&admin,witness.clone())?.is_none(),"active case must be the blocking obligation");
            let control=sessions::register(tx,sessions::NewSession{crew_id:crew,role:Role::Admin,executor:"claude".into(),title:Some("idle control".into()),..Default::default()})?;
            sessions::set_desired(tx,&control.id,Desired::Running)?;
            crewd_store::sessions::set_agent_session_id(tx.conn(),&control.id,"retained-unbound-control")?;
            assert!(crewd_collab::session_retirement::prepare(tx,&control.id,witness)?.is_some(),"same basic idle facts on unbound Admin must qualify");
            Ok(())
        })?;
        Ok::<_,crewd_collab::CollabError>(())
    }).await.unwrap().unwrap();
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["effectsCount"],
        0
    );
    r.close().await;
}

async fn wait_started(r: &Rig, case: &str, session: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let sid = session.to_owned();
            let desired =
                r.f.state
                    .db
                    .call_result(move |s| sessions::get(s.conn(), &sid).map(|s| s.desired))
                    .await
                    .unwrap()
                    .unwrap();
            if desired == Desired::Running
                && view(&r.f, case).await["executionRecord"]["phase"] == "running"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("physical Spawned must persist both Running case and Running session");
}

#[path = "admin_outcome.rs"]
mod outcome;

#[path = "admin_retirement.rs"]
mod retirement;

#[path = "admin_fence_review.rs"]
mod admin_fence_review;
#[path = "admin_native_policy.rs"]
mod admin_native_policy;
#[path = "admin_native_tools.rs"]
mod admin_native_tools;
#[path = "admin_handoff.rs"]
mod handoff;
