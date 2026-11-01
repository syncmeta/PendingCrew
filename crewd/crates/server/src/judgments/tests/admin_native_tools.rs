//! Opt-in actual native Codex -> built crew MCP -> real case RPC/transaction proof.
//! The first proof uses StandIn case birth and a separate actual native driver.
//! The second routes case launches through the production Codex adapter and
//! verifies the real Core authority fence; ordinary sources remain StandIn.
use super::*;
use crewd_exec::{
    codex::{
        driver::{CodexConfig, CodexSpec, CodexStart, start_codex},
        profile::NativePolicy,
    },
    launch::CommsSpec,
    resume::MemoryLedger,
    session::SessionEvent,
};
use std::path::PathBuf;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "actual local Codex and built crewd helper; scripts/dev/run-admin-native-tools.py"]
async fn real_native_case_tools_scope_and_factual_close() {
    assert_eq!(
        std::env::var("PCW_ADMIN_NATIVE_PROBE").as_deref(),
        Ok("loopback")
    );
    let evidence = PathBuf::from(std::env::var_os("PCW_ADMIN_NATIVE_EVIDENCE").unwrap());
    let project = evidence.join("project");
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
                    title: "FOREIGN_NATIVE_CASE_SENTINEL".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
    let rig = Rig::new(f).await;
    let sid = rig.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&rig, &source.id, &sid).await;
    let before = current_work(&rig.f, &source.work).await;
    let token = rig.f.state.tokens.mint(&sid);
    std::fs::write(evidence.join("fixture.json"),serde_json::to_vec_pretty(&json!({
        "session":sid,"caseId":source.id,"work":source.work,"revision":before.revision,
        "foreign":foreign.id,"foreignRevision":foreign.revision,
        "boundary":"StandIn Core binding; independent real native driver -> real helper -> RPC -> Tx"
    })).unwrap()).unwrap();
    let listener = tokio::net::UnixListener::bind(rig.f.dir.path().join("crewd.sock")).unwrap();
    let state = rig.f.state.clone();
    let serving = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept()=>{
                    let Ok((socket,_))=accepted else {break};
                    let state=state.clone();
                    connections.spawn(async move {crate::conn::run(state,socket).await});
                },
                Some(_)=connections.join_next(),if !connections.is_empty()=>{},
            }
        }
    });
    let started = start_codex(
        CodexStart {
            session_id: sid.clone(),
            program: PathBuf::from(std::env::var_os("PCW_ADMIN_NATIVE_PROGRAM").unwrap()),
            child_path: std::env::var("PATH").unwrap(),
            spec: CodexSpec {
                workdir: project,
                native_policy: NativePolicy::AdminCaseRestricted,
                strict_resume: true,
                comms: Some(CommsSpec {
                    helper: PathBuf::from(std::env::var_os("PCW_ADMIN_NATIVE_HELPER").unwrap()),
                    data_dir: rig.f.dir.path().to_owned(),
                    session_id: sid.clone(),
                    token,
                    hooks: false,
                    managed_hooks: false,
                }),
                initial_prompt: Some(
                    "Run the isolated case collaboration acceptance sequence.".into(),
                ),
                ..Default::default()
            },
            ledger: Arc::new(MemoryLedger::default()),
        },
        CodexConfig {
            runtime_dir: evidence.join("runtime"),
            ..Default::default()
        },
    )
    .await;
    let (handle, mut events) = match started {
        Ok(v) => v,
        Err(error) => {
            serving.abort();
            rig.close().await;
            panic!("actual restricted native driver did not start: {error}")
        }
    };
    let mut items = Vec::new();
    let completed = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::Item { item, .. } => items.push(item),
                SessionEvent::TurnEnded { .. } => return true,
                SessionEvent::Exited(_) => return false,
                _ => {}
            }
        }
        false
    })
    .await;
    let _ = handle.stop();
    let reaped = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = events.recv().await {
            if matches!(event, SessionEvent::Exited(_)) {
                return true;
            }
        }
        false
    })
    .await;
    let after = current_work(&rig.f, &source.work).await;
    let foreign_after = current_work(&rig.f, &foreign.id).await;
    let result = view(&rig.f, &source.id).await;
    let events = rig
        .f
        .state
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
    std::fs::write(
        evidence.join("native-items.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    std::fs::write(evidence.join("receipt.json"),serde_json::to_vec_pretty(&json!({
        "after":after,"foreignAfter":foreign_after,"case":result,"events":events,
        "nativeReaped":reaped.as_ref().is_ok_and(|v|*v),"turnCompleted":completed.as_ref().is_ok_and(|v|*v)
    })).unwrap()).unwrap();
    serving.abort();
    let _ = serving.await;
    rig.close().await;
    assert_eq!(
        completed.ok(),
        Some(true),
        "native turn must actually complete"
    );
    assert_eq!(reaped.ok(), Some(true), "native process must be reaped");
    assert!(
        items
            .iter()
            .any(|i| i["type"] == "mcpToolCall" && i["server"] == "crew"),
        "no real crew MCP call witnessed"
    );
    assert_eq!(serde_json::to_value(&after).unwrap()["status"], "done");
    assert_ne!(after.revision, before.revision);
    assert_eq!(
        foreign_after.revision, foreign.revision,
        "foreign work changed"
    );
    assert_eq!(
        result["executionRecord"]["effectsCount"], 1,
        "only one verified case effect is allowed"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains(&sid) && e.contains(&source.work)),
        "actual Work event must be attributed to the case Admin"
    );
}

#[derive(Default)]
struct NativeJournal {
    requested_model: Option<String>,
    observed_model: Option<String>,
    pid: Option<u32>,
    ended: bool,
    exited: bool,
    items: Vec<serde_json::Value>,
}
struct CaseNativeExecutor {
    ordinary: StandIn,
    native: crate::orchestra::exec_codex::CodexExecutor,
    journal: Arc<std::sync::Mutex<NativeJournal>>,
}
impl crate::orchestra::exec::Executor for CaseNativeExecutor {
    fn supports_native_policy(
        &self,
        policy: crate::orchestra::exec::NativeExecutionPolicy,
    ) -> bool {
        self.native.supports_native_policy(policy)
    }
    fn accepts_initial_context(&self) -> bool {
        true
    }
    fn launch(
        &self,
        req: crate::orchestra::exec::LaunchRequest,
    ) -> crate::orchestra::exec::BoxFut<
        '_,
        Result<crate::orchestra::exec::Launched, crate::orchestra::exec::LaunchError>,
    > {
        use crate::orchestra::exec::NativeExecutionPolicy;
        Box::pin(async move {
            if req.native_policy == NativeExecutionPolicy::Ordinary {
                return self.ordinary.launch(req).await;
            }
            self.journal.lock().unwrap().requested_model = req.model.clone();
            let (handle, mut events) = self.native.launch(req).await?;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let journal = self.journal.clone();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    {
                        let mut observed = journal.lock().unwrap();
                        match &event {
                            SessionEvent::Spawned { pid, .. } => observed.pid = Some(*pid),
                            SessionEvent::TurnEnded { .. } => observed.ended = true,
                            SessionEvent::Profile(profile) => {
                                observed.observed_model = profile.model.clone()
                            }
                            SessionEvent::Exited(_) => observed.exited = true,
                            SessionEvent::Item { item, .. } => observed.items.push(item.clone()),
                            _ => {}
                        }
                    }
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            });
            Ok((handle, rx))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "actual Core/production Codex adapter physical stop; run-admin-native-tools.py --core-fence"]
async fn real_core_native_case_source_revocation_stops_physical_process() {
    use crate::orchestra::exec_codex::{CodexExecutor, CodexExecutorConfig, CodexProgram};
    assert_eq!(
        std::env::var("PCW_ADMIN_NATIVE_PROBE").as_deref(),
        Ok("loopback")
    );
    let evidence = PathBuf::from(std::env::var_os("PCW_ADMIN_NATIVE_EVIDENCE").unwrap());
    let project = evidence.join("project");
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let crew = f.crew.clone();
    let wd = project.to_string_lossy().to_string();
    f.state
        .collab(move |tx| {
            crews::update(
                tx,
                &crew,
                crews::CrewPatch {
                    working_directory: Some(wd.clone()),
                    ..Default::default()
                },
            )?;
            // Test-only initial source setup, before observation or decision: leave
            // model/effort unset so real Profile adoption cannot be masked.
            tx.conn().execute(
                "UPDATE sessions SET working_directory=?2,executor='codex' WHERE crew_id=?1",
                rusqlite::params![crew, wd],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let case = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &case))
        .await
        .unwrap()
        .unwrap();
    let original_work = current_work(&f, &source.work).await;
    std::fs::write(
        evidence.join("fixture.json"),
        serde_json::to_vec(&json!({"mode":"core","work":source.work,"caseId":source.id,"revision":original_work.revision})).unwrap(),
    )
    .unwrap();
    let listener = tokio::net::UnixListener::bind(f.dir.path().join("crewd.sock")).unwrap();
    let state = f.state.clone();
    let serving = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept()=>{let Ok((socket,_))=accepted else{break};let state=state.clone();connections.spawn(async move{crate::conn::run(state,socket).await});},
                Some(_)=connections.join_next(),if !connections.is_empty()=>{},
            }
        }
    });
    let journal = Arc::new(std::sync::Mutex::new(NativeJournal::default()));
    let ordinary = StandIn::default();
    let registry = Arc::new(
        crewd_exec::registry::ProcessRegistry::load(&evidence.join("native-registry.json"))
            .unwrap(),
    );
    let routed = CaseNativeExecutor {
        ordinary: ordinary.clone(),
        journal: journal.clone(),
        native: CodexExecutor::new(CodexExecutorConfig {
            program: CodexProgram::Path(PathBuf::from(
                std::env::var_os("PCW_ADMIN_NATIVE_PROGRAM").unwrap(),
            )),
            helper: PathBuf::from(std::env::var_os("PCW_ADMIN_NATIVE_HELPER").unwrap()),
            data_dir: f.dir.path().to_owned(),
            runtime_dir: evidence.join("runtime"),
            registry: Some(registry),
            extra_env: Default::default(),
            child_path: Some(std::env::var("PATH").unwrap()),
        }),
    };
    let rx = f.state.hub.subscribe_collab();
    let mut cfg = orchestra::OrchestraConfig::for_tests();
    cfg.admission = Some(f.state.admission.clone());
    let (handle, actor) = orchestra::spawn(
        Arc::new(DbPort::new(
            f.state.clone(),
            f.dir.path().join(crewd_store::DB_FILE),
        )),
        Executors::new().with("codex", Arc::new(routed)),
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
    let rig = Rig {
        f,
        handle,
        _actor: actor,
        bridge,
        exec: ordinary,
    };
    let sid = rig.handle.start_admin_case(permit).await.unwrap().id;
    std::fs::write(
        evidence.join("fixture.json"),
        serde_json::to_vec(
            &json!({"mode":"core","session":sid,"work":source.work,"caseId":source.id,"revision":original_work.revision}),
        )
        .unwrap(),
    )
    .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            {
                let j = journal.lock().unwrap();
                if j.exited {
                    return false;
                }
                if j.pid.is_some() && j.ended {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or(false);
    // Allow the natural one-second authority guard to inspect the legitimate
    // close checkpoint before changing source admission.
    tokio::time::sleep(Duration::from_millis(1250)).await;
    let closed_work = current_work(&rig.f, &source.work).await;
    let work_events = rig
        .f
        .state
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
    let closure_actor_verified = work_events.iter().any(|event| {
        let event: serde_json::Value = serde_json::from_str(event).unwrap();
        event["by"]["kind"] == "scopedsession"
            && event["by"]["id"] == sid
            && event["by"]["authority"]["roles"][&rig.f.crew] == "admin"
            && event["kind"]["type"] == "done"
            && event["kind"]["id"] == source.work
            && event["kind"]["evidence"]["kind"] == "text"
            && event["kind"]["evidence"]["value"]
                == "isolated native integration fixture verified closure"
    });
    let before = view(&rig.f, &source.id).await;
    let session = sid.clone();
    let effective_model = rig
        .f
        .state
        .db
        .call_result(move |s| sessions::get(s.conn(), &session).map(|s| s.model))
        .await
        .unwrap()
        .unwrap();
    let profile_adopted_from_unset = {
        let observed = journal.lock().unwrap();
        observed.requested_model.is_none()
            && observed.observed_model.is_some()
            && effective_model == observed.observed_model
    };
    let pid = journal.lock().unwrap().pid;
    let live_before = pid.is_some_and(|pid| {
        matches!(
            crewd_exec::registry::probe(pid as i32),
            crewd_exec::registry::ProbeResult::Found(_)
        )
    });
    // The only intervention: revoke source admission. Do not call stop or
    // shutdown until the real Core fence has independently produced Exited.
    rig.f.state.admission.revoke(&source.owner);
    let fenced = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let exited = journal.lock().unwrap().exited;
            if exited && !rig.handle.live(&sid).await.unwrap().running {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or(false);
    let physically_gone = pid.is_some_and(|pid| {
        matches!(
            crewd_exec::registry::probe(pid as i32),
            crewd_exec::registry::ProbeResult::Missing
        )
    });
    let revoked = !rig.f.state.admission.contains(&sid);
    let after = view(&rig.f, &source.id).await;
    let items = journal.lock().unwrap().items.clone();
    std::fs::write(evidence.join("core-fence-receipt.json"),serde_json::to_vec_pretty(&json!({"ready":ready,"closedWork":closed_work,"closureActorVerified":closure_actor_verified,"workEvents":work_events,"profileAdoptedFromUnset":profile_adopted_from_unset,"effectiveModel":effective_model,"pid":pid,"liveBeforeRevocation":live_before,"fencedBeforeCleanup":fenced,"physicallyGoneBeforeCleanup":physically_gone,"adminAdmissionRevoked":revoked,"before":before,"after":after,"items":items,"boundary":"real Core -> routing test executor -> production Codex adapter -> real native and crew helper; ordinary sources remain StandIn"})).unwrap()).unwrap();
    serving.abort();
    let _ = serving.await;
    rig.close().await;
    assert_eq!(
        serde_json::to_value(&closed_work).unwrap()["status"],
        "done",
        "actual native close must commit to the original DB"
    );
    assert_ne!(closed_work.revision, original_work.revision);
    assert_eq!(
        before["executionRecord"]["effectsCount"], 1,
        "exactly one verified native case effect"
    );
    assert!(
        closure_actor_verified,
        "closure event must carry the case Admin and actual supplied evidence"
    );
    assert!(
        profile_adopted_from_unset,
        "actual Profile must adopt model from unset without self-fencing"
    );
    assert!(
        ready && live_before,
        "actual native case self-fenced or failed before source revocation"
    );
    assert!(
        items
            .iter()
            .any(|i| i["type"] == "mcpToolCall" && i["server"] == "crew"),
        "first real case tool did not run"
    );
    assert!(
        fenced && physically_gone && revoked,
        "real Core authority fence did not stop physical native process before cleanup"
    );
}
