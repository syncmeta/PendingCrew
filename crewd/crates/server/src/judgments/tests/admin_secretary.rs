//! Real stored Human source -> original native Jev HTTP -> Core case factory ->
//! authenticated socket -> case tool transaction. The executor is StandIn;
//! this is a production-path fixture, not actual native/GUI acceptance.
use super::*;
use crewd_collab::{admin_secretary as secretary, asks};
use crewd_proto::admin_secretary::{
    HUMAN_GET, HUMAN_REQUEST, PUBLISH_TOOL, RECEIPT_TOOL, SNAPSHOT_TOOL,
};

async fn source(f: &Fixture) -> (String, String) {
    let crew = f.crew.clone();
    let worker = f.worker.clone();
    f.state
        .collab(move |tx| {
            crewd_store::admin_secretary::validate_public(tx.conn())?;
            let captain = crews::captain_of(tx.conn(), &crew)?.unwrap();
            sessions::set_desired(tx, &captain, Desired::Running)?;
            // Establish the real completed source before the writing intent;
            // an unrelated first-observation transition must not race this
            // controlled activity/transport fixture.
            let mut completed = false;
            for _ in 0..100 {
                crewd_collab::patrol_runtime::tick(tx)?;
                if crewd_collab::patrol_runtime::status(tx.conn(), &crew)?
                    .run
                    .is_some_and(|r| r.completed_at_ms.is_some())
                {
                    completed = true;
                    break;
                }
            }
            assert!(
                completed,
                "original periodic producer must establish completion"
            );
            secretary::observe(tx, &crew)?;
            let ask = asks::open(
                tx,
                &worker,
                asks::OpenAsk {
                    question: "Synthetic original Human question. Do not answer it for Human."
                        .into(),
                    default: None,
                    resume_note: None,
                },
            )?;
            secretary::observe(tx, &crew)?;
            let view = secretary::human_get(tx.conn(), &crew, tx.now_ms())?;
            Ok((view.case.unwrap().id, ask.ask))
        })
        .await
        .unwrap()
}
async fn explicit_view(f: &Fixture) -> serde_json::Value {
    f.rpc(HUMAN_GET, json!({"crewId":f.crew})).await.unwrap()
}
async fn ready() -> (
    Fixture,
    String,
    String,
    execution::AdminLaunchPermit,
    FakeJev,
) {
    ready_with(vec![answer(0.99)]).await
}
async fn ready_with(
    replies: Vec<Reply>,
) -> (
    Fixture,
    String,
    String,
    execution::AdminLaunchPermit,
    FakeJev,
) {
    let fake = FakeJev::start(replies).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let (id, h) = source(&f).await;
    f.runtime
        .run_admin_case(f.crew.clone(), id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let cid = id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &cid))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fake.requests().len(), 1);
    assert_eq!(
        explicit_view(&f).await["case"]["decisionRecord"]["adopted"],
        true
    );
    (f, id, h, permit, fake)
}
async fn wait(r: &Rig, id: &str, sid: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            r.handle.latches().await.unwrap();
            let view = explicit_view(&r.f).await;
            if view["case"]["id"] == id
                && view["case"]["execution"] == "running"
                && view["case"]["executionRecord"]["sessionId"] == sid
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn call(
    socket: &mut Socket,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, crewd_proto::RpcError> {
    socket
        .call("agent.call", json!({"tool":name,"args":args}))
        .await
}
fn decoded(value: serde_json::Value) -> serde_json::Value {
    let actual: crewd_proto::agent::CallResult =
        serde_json::from_value(value).expect("original agent.call typed result");
    assert!(!actual.is_error, "{}", actual.text);
    serde_json::from_str(&actual.text)
        .expect("Secretary JSON carried by the original typed result text")
}

#[tokio::test]
async fn secretary_native_judgment_factory_authenticated_tools_public_receipt_and_original_h() {
    let (f, id, h, permit, _fake) = ready().await;
    let r = Rig::new(f).await;
    let (a, b) = tokio::join!(
        r.handle.start_admin_case(permit.clone()),
        r.handle.start_admin_case(permit)
    );
    assert!(a.is_ok() || b.is_ok(), "{a:?}; {b:?}");
    let sid = a.or(b).unwrap().id;
    wait(&r, &id, &sid).await;
    acknowledge_initial_input(&r, &sid).await;
    assert_eq!(r.exec.launches(&sid).len(), 1);
    let port = DbPort::new(r.f.state.clone(), r.f.dir.path().join(crewd_store::DB_FILE));
    let world = crate::orchestra::port::Port::worldview(&port, &sid)
        .await
        .unwrap();
    assert!(world.contains("admin_secretary_snapshot") && !world.contains("work_item_close"));
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    let tools = socket.call("agent.tools", json!({})).await.unwrap();
    let actual: std::collections::BTreeSet<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        actual,
        std::collections::BTreeSet::from([SNAPSHOT_TOOL, PUBLISH_TOOL, RECEIPT_TOOL])
    );
    for tool in [
        "post",
        "work_item_get",
        "work_item_close",
        "admin_case_handoff_request",
        "admin_case_custody_list",
        "data_workspace_get",
    ] {
        let result = call(&mut socket, tool, json!({"id":"W1","text":"forbidden"})).await;
        assert!(
            result.is_err() || result.as_ref().is_ok_and(|v| v["isError"] == true),
            "{tool}: {result:?}"
        );
    }
    assert!(
        socket
            .call(HUMAN_GET, json!({"crewId":r.f.crew}))
            .await
            .is_err()
    );
    let snapshot = decoded(call(&mut socket, SNAPSHOT_TOOL, json!({})).await.unwrap());
    assert_eq!(snapshot["caseId"], id);
    assert_eq!(snapshot["author"]["sessionId"], sid);
    assert_eq!(snapshot["facts"]["humanWaits"][0]["askId"], h);
    let crew = r.f.crew.clone();
    let cid = id.clone();
    r.f.state
        .collab(move |tx| {
            let captain = crews::captain_of(tx.conn(), &crew)?.unwrap();
            sessions::set_working(tx, &captain, true)?;
            sessions::set_working(tx, &captain, false)?;
            messages::post(
                tx,
                Sender::Session(captain.clone()),
                messages::NewPost {
                    channel_id: crewd_store::channels::lobby_id(&crew),
                    text: "ordinary captain output while secretary writes".into(),
                    ..Default::default()
                },
            )?;
            sessions::note_progress(tx, &captain, crewd_collab::ProgressKind::ToolCall)?;
            let witness = execution::binding(tx.conn(), &crew, &cid)?.unwrap();
            assert!(matches!(
                execution::native_lease(tx.conn(), &witness, tx.now_ms())?,
                execution::NativeLeaseState::Active(_)
            ));
            Ok(())
        })
        .await
        .unwrap();
    let after_activity = decoded(call(&mut socket, SNAPSHOT_TOOL, json!({})).await.unwrap());
    assert_eq!(
        after_activity, snapshot,
        "real activity and output cannot replace or revoke the original author generation"
    );
    assert_eq!(
        decoded(
            call(
                &mut socket,
                RECEIPT_TOOL,
                json!({"request_id":uuid::Uuid::new_v4().to_string()})
            )
            .await
            .unwrap()
        ),
        serde_json::Value::Null
    );

    let request = uuid::Uuid::new_v4().to_string();
    let args = json!({"request_id":request,"source_revision":snapshot["revision"],"generation":snapshot["author"]["generation"],"body":"原问题仍待人确认；这份简报只整理来源，不代答。","references":snapshot["references"]});
    let publication = decoded(call(&mut socket, PUBLISH_TOOL, args.clone()).await.unwrap());
    let retry = decoded(call(&mut socket, PUBLISH_TOOL, args).await.unwrap());
    assert_eq!(publication, retry);
    let receipt = decoded(
        call(&mut socket, RECEIPT_TOOL, json!({"request_id":request}))
            .await
            .unwrap(),
    );
    assert_eq!(receipt, publication);
    let human = explicit_view(&r.f).await;
    assert_eq!(human["state"], "current");
    assert_eq!(human["publication"], publication);
    let crew = r.f.crew.clone();
    let original = h.clone();
    let open =
        r.f.state
            .db
            .call_result(move |s| asks::get(s.conn(), &crew, &original))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(open.state, crewd_proto::AskStateView::Open);
    let list =
        r.f.rpc("adminCase.list", json!({"crewId":r.f.crew}))
            .await
            .unwrap();
    assert!(list["items"].as_array().unwrap().is_empty());
    assert!(
        r.f.rpc("adminCase.get", json!({"crewId":r.f.crew,"id":id}))
            .await
            .is_err()
    );
    let facts =
        r.f.rpc("adminCase.humanBrief", json!({"crewId":r.f.crew}))
            .await
            .unwrap();
    assert!(facts["cases"].as_array().unwrap().is_empty());
    assert!(
        r.f.rpc(
            HUMAN_REQUEST,
            json!({"crewId":r.f.crew,"expectedRevision":snapshot["revision"]})
        )
        .await
        .is_err(),
        "only original public-command receipt can mutate"
    );
    // Original public command ownership, UUID conflict and replay surround the
    // entire Human request; raw administrative RPC never becomes a write alias.
    let database =
        r.f.state
            .db
            .call_result(|s| crewd_store::meta_get(s.conn(), "dbId"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    let outer = uuid::Uuid::new_v4().to_string();
    let submit = json!({"requestId":outer,"dbId":database,"runtimeBindingId":r.f.state.runtime_binding_id,"command":{"operation":HUMAN_REQUEST,"params":{"crewId":r.f.crew,"expectedRevision":snapshot["revision"]}}});
    let committed = r.f.rpc("command.submit", submit.clone()).await.unwrap();
    assert_eq!(
        r.f.rpc("command.submit", submit.clone()).await.unwrap(),
        committed
    );
    let mut conflict = submit;
    conflict["command"]["params"]["expectedRevision"] = json!("0".repeat(64));
    assert!(r.f.rpc("command.submit", conflict).await.is_err());
    let crew = r.f.crew.clone();
    r.f.state
        .collab(move |tx| {
            crewd_store::judgment_state::restored(tx.conn())?;
            secretary::observe(tx, &crew)
        })
        .await
        .unwrap();
    for (tool, args) in [
        (SNAPSHOT_TOOL, json!({})),
        (RECEIPT_TOOL, json!({"request_id":request})),
    ] {
        let denied = call(&mut socket, tool, args).await;
        assert!(
            denied.is_err() || denied.as_ref().is_ok_and(|v| v["isError"] == true),
            "restored Scope must not read old bound facts or receipt"
        );
    }
    socket.close().await;
    r.close().await;
}
#[tokio::test]
async fn secretary_low_confidence_and_no_native_configuration_never_launch() {
    for probability in [0.01, 0.49] {
        let fake = FakeJev::start(vec![answer(probability)]).await;
        let f = Fixture::new().await;
        f.configure(&fake.base_url).await;
        let (id, _) = source(&f).await;
        f.runtime
            .run_admin_case(f.crew.clone(), id.clone(), true)
            .await
            .unwrap();
        let crew = f.crew.clone();
        let permit = f
            .state
            .collab(move |tx| execution::launch_permit(tx, &crew, &id))
            .await
            .unwrap();
        assert!(permit.is_none());
        assert_ne!(
            explicit_view(&f).await["case"]["decisionRecord"]["adopted"],
            true
        );
        assert_eq!(fake.requests().len(), 1);
        f.close().await;
    }
    let f = Fixture::new().await;
    let (id, _) = source(&f).await;
    f.runtime
        .run_admin_case(f.crew.clone(), id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    assert!(
        f.state
            .collab(move |tx| execution::launch_permit(tx, &crew, &id))
            .await
            .unwrap()
            .is_none()
    );
    f.close().await;
}

#[tokio::test]
async fn original_stop_internal_worldview_and_wrong_case_binding_remain_fenced() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    let port = DbPort::new(r.f.state.clone(), r.f.dir.path().join(crewd_store::DB_FILE));
    let world = crate::orchestra::port::Port::worldview(&port, &sid)
        .await
        .unwrap();
    assert!(world.contains("work_item_get") && !world.contains("admin_secretary_snapshot"));
    let index = format!("admin.case.execution.session.v1:{sid}");
    let k = index.clone();
    let original =
        r.f.state
            .db
            .call_result(move |s| {
                let original = crewd_store::meta_get(s.conn(), &k)?.unwrap();
                crewd_store::meta_set(s.conn(), &k, "AC00000000000000000099")?;
                Ok::<_, crewd_collab::CollabError>(original)
            })
            .await
            .unwrap()
            .unwrap();
    assert!(
        crate::orchestra::port::Port::worldview(&port, &sid)
            .await
            .is_err(),
        "wrong case index cannot fall back to ordinary unrestricted worldview"
    );
    r.f.state
        .db
        .call_result(move |s| crewd_store::meta_set(s.conn(), &index, &original))
        .await
        .unwrap()
        .unwrap();
    r.close().await;
}

async fn idle_witness(r: &Rig, sid: &str) -> crewd_collab::session_retirement::ActorWitness {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some((_, w)) = r
                .handle
                .retirement_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|(s, _)| s == sid)
            {
                break w;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("original live Core idle witness")
}
async fn retirement_phase(
    r: &Rig,
    sid: &str,
    want: crewd_proto::RetirementPhase,
) -> crewd_proto::RetirementView {
    let crew = r.f.crew.clone();
    let sid = sid.to_owned();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let (crew, id) = (crew.clone(), sid.clone());
            let record =
                r.f.state
                    .db
                    .call_result(move |s| {
                        crewd_collab::session_retirement::status(s.conn(), &crew, &id)
                    })
                    .await
                    .unwrap()
                    .unwrap()
                    .record;
            if let Some(v) = record
                && v.phase == want
            {
                break v;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("original retirement phase")
}
#[tokio::test]
async fn secretary_publication_requires_actual_exited_then_reopens_without_author_relaunch_or_h_answer()
 {
    use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
    use crewd_proto::RetirementPhase;
    let retiring = Reply::ok(
        json!({"model":"fake-native","answers":{"can_retire":{"type":"noul","noul":0.99}}})
            .to_string(),
    );
    let (f, id, h, permit, fake) = ready_with(vec![answer(0.99), retiring]).await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait(&r, &id, &sid).await;
    acknowledge_initial_input(&r, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    let snapshot = decoded(call(&mut socket, SNAPSHOT_TOOL, json!({})).await.unwrap());
    let request = uuid::Uuid::new_v4().to_string();
    let publication=decoded(call(&mut socket,PUBLISH_TOOL,json!({"request_id":request,"source_revision":snapshot["revision"],"generation":snapshot["author"]["generation"],"body":"原问题仍由人确认；本次只完成简报写作。","references":snapshot["references"]})).await.unwrap());
    let crew = r.f.crew.clone();
    let cid = id.clone();
    let outcome =
        r.f.state
            .collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &cid))
            .await
            .unwrap();
    assert_eq!(outcome.phase, crewd_proto::AdminCaseOutcomePhase::Resolved);
    let admin = sid.clone();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_store::sessions::set_agent_session_id(
                s.conn(),
                &admin,
                "retained-secretary-native-identity",
            )
        })
        .await
        .unwrap()
        .unwrap();
    r.exec.emit(&sid, SessionEvent::Working(true));
    r.exec.emit(&sid, SessionEvent::Working(false));
    r.handle.latches().await.unwrap();
    r.exec.lock().hang_on_stop.insert(sid.clone());
    r.f.runtime
        .run_retirement(sid.clone(), idle_witness(&r, &sid).await, true)
        .await
        .unwrap();
    let pending = retirement_phase(&r, &sid, RetirementPhase::ExitRequested).await;
    assert!(!pending.adopted);
    assert!(
        r.handle.live(&sid).await.unwrap().running,
        "saved prose and stop request do not mean actual exit"
    );
    assert!(
        call(&mut socket, RECEIPT_TOOL, json!({"request_id":request}))
            .await
            .is_err(),
        "the original actual retirement revokes this attached author"
    );
    assert!(!r.f.state.admission.contains(&sid));
    socket.close().await;
    assert_eq!(r.exec.lock().stops.iter().filter(|s| *s == &sid).count(), 1);
    r.exec.emit(
        &sid,
        SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(0),
            stopped_by_user: false,
            hit_limit: false,
        }),
    );
    assert!(
        retirement_phase(&r, &sid, RetirementPhase::Parked)
            .await
            .adopted
    );
    let visible = explicit_view(&r.f).await;
    assert_eq!(visible["publication"], publication);
    assert_eq!(visible["case"]["retirement"], "parked");
    let crew = r.f.crew.clone();
    let original = h.clone();
    assert_eq!(
        r.f.state
            .db
            .call_result(move |s| asks::get(s.conn(), &crew, &original))
            .await
            .unwrap()
            .unwrap()
            .state,
        crewd_proto::AskStateView::Open
    );
    assert_eq!(fake.requests().len(), 2);
    r.exec.lock().hang_on_stop.remove(&sid);
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
    _actor.finished().await.unwrap();
    drop((handle, bridge, exec));
    f.reopen().await;
    f.reopen().await;
    let reopened = Rig::new(f).await;
    reopened.handle.latches().await.unwrap();
    assert!(reopened.exec.launches(&sid).is_empty());
    let after = explicit_view(&reopened.f).await;
    assert_eq!(after["source"]["sourceId"], snapshot["sourceId"]);
    assert_eq!(after["publication"], publication);
    assert_eq!(after["case"]["retirement"], "parked");
    let crew = reopened.f.crew.clone();
    assert_eq!(
        reopened
            .f
            .state
            .db
            .call_result(move |s| asks::get(s.conn(), &crew, &h))
            .await
            .unwrap()
            .unwrap()
            .state,
        crewd_proto::AskStateView::Open
    );
    let count = reopened
        .f
        .state
        .db
        .call_result(|s| {
            s.conn()
                .query_row("SELECT count(*) FROM admin_secretary_sources", [], |r| {
                    r.get::<_, i64>(0)
                })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fake.requests().len(),
        2,
        "reopen must not repeat the original native decision or writing intent"
    );
    reopened.close().await;
}
