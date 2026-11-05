//! Actual administrative decision Runtime with the shared local fake HTTP provider.
//! Source construction uses original stop-check transactions; no Admin runner is started.
use super::*;
use crewd_collab::{admin_cases as cases, stop_checks, wakes, work};
use crewd_proto::{
    AdminCaseGetParams, AdminCaseListParams, Desired, WorkCreateParams, WorkUpdateParams,
};
use std::time::Duration;
const LIMIT: Duration = Duration::from_secs(15);
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
async fn answered_view(f: &Fixture, id: &str) -> serde_json::Value {
    // The periodic runtime may claim the source before the explicit call. The
    // caller then observes the peer's durable result, rather than assuming the
    // returned call waited for a request owned by another task.
    tokio::time::timeout(LIMIT, async {
        loop {
            let current = view(f, id).await;
            if current["decisionRecord"]["phase"] == "answered" {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the original judgment must reach its durable answered phase")
}
async fn counts(f: &Fixture) -> (i64, i64) {
    f.state
        .db
        .call_result(|s| {
            Ok::<_, crewd_store::StoreError>((
                s.conn().query_row(
                    "SELECT count(*) FROM sessions WHERE role='admin'",
                    [],
                    |r| r.get(0),
                )?,
                s.conn()
                    .query_row("SELECT count(*) FROM wakes", [], |r| r.get(0))?,
            ))
        })
        .await
        .unwrap()
        .unwrap()
}
async fn wait_request(fake: &FakeJev) {
    tokio::time::timeout(LIMIT, async {
        while fake.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn unconfigured_and_unadmitted_sources_never_call_provider() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let v = view(&f, &source.id).await;
    assert_eq!(v["decisionRecord"]["reason"], "unconfigured");
    assert!(fake.requests().is_empty());
    f.close().await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    f.state.admission.revoke(&f.worker);
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let v = view(&f, &source.id).await;
    assert_eq!(v["decisionRecord"]["phase"], "waiting_admission");
    assert_eq!(v["decisionRecord"]["adopted"], false);
    assert!(fake.requests().is_empty());
    f.close().await;
}
#[tokio::test]
async fn native_high_and_low_decisions_are_persisted_without_admin_or_delivery_effects() {
    for p in [0.99, 0.01] {
        let fake = FakeJev::start(vec![answer(p)]).await;
        let f = Fixture::new().await;
        f.configure(&fake.base_url).await;
        let source = sources(&f, 1).await.remove(0);
        let before = counts(&f).await;
        f.runtime
            .run_admin_case(f.crew.clone(), source.id.clone(), true)
            .await
            .unwrap();
        // The periodic runtime can claim this source before the explicit
        // call. In that case the call returns without waiting for its peer.
        let v = tokio::time::timeout(LIMIT, async {
            loop {
                let current = view(&f, &source.id).await;
                if !matches!(
                    current["decisionRecord"]["phase"].as_str(),
                    Some("prepared" | "dispatched") | None
                ) {
                    break current;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the original judgment should reach a durable terminal phase");
        assert_eq!(v["decisionRecord"]["phase"], "answered", "{v}");
        assert_eq!(v["decisionRecord"]["probability"], p);
        assert_eq!(v["decisionRecord"]["adopted"], p >= 0.5);
        assert_eq!(v["execution"], "not_started");
        assert_eq!(v["retirement"], "not_started");
        let public = f
            .rpc("adminCase.get", json!({"crewId":f.crew,"id":source.id}))
            .await
            .unwrap();
        assert_eq!(
            public, v,
            "authenticated public read must expose the actual decision receipt"
        );
        assert_eq!(counts(&f).await, before);
        let wid = source.work.clone();
        let plan = f
            .state
            .db
            .call_result(move |s| work::get(s.conn(), &wid))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(plan.status, "open");
        f.runtime
            .run_admin_case(f.crew.clone(), source.id.clone(), true)
            .await
            .unwrap();
        assert_eq!(fake.requests().len(), 1);
        f.close().await;
    }
}
#[tokio::test]
async fn late_fact_configuration_stop_and_restore_reject_provider_result() {
    for mutation in ["fact", "config", "stop", "restore", "admission"] {
        let fake = FakeJev::start(vec![answer(0.99).delayed(Duration::from_millis(300))]).await;
        let f = Fixture::new().await;
        f.configure(&fake.base_url).await;
        let source = sources(&f, 1).await.remove(0);
        let runtime = f.runtime.clone();
        let crew = f.crew.clone();
        let id = source.id.clone();
        let task =
            tokio::spawn(async move { runtime.run_admin_case(crew, id, true).await.unwrap() });
        wait_request(&fake).await;
        match mutation {
            "fact" => {
                let id = source.work.clone();
                f.state
                    .collab(move |tx| {
                        let w = work::get(tx.conn(), &id)?;
                        work::update(
                            tx,
                            &Sender::Human,
                            WorkUpdateParams {
                                id,
                                expected_revision: w.revision,
                                note: Some("new factual report after model snapshot".into()),
                                ..Default::default()
                            },
                        )
                    })
                    .await
                    .unwrap();
            }
            "config" => {
                let current = f
                    .state
                    .db
                    .call_result(|store| crewd_collab::judgments::configuration(store.conn()))
                    .await
                    .unwrap()
                    .unwrap();
                let result = f
                    .rpc(
                        SETTINGS_SET,
                        serde_json::to_value(JudgmentSettingsSet {
                            request_id: uuid::Uuid::new_v4().to_string(),
                            expected_revision: current.revision,
                            config: Some(Fixture::config(&fake.base_url)),
                            credential_update: CredentialUpdate::Keep,
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
                let changed: JudgmentSettingsResult = serde_json::from_value(result).unwrap();
                assert_eq!(changed.outcome, JudgmentConfigurationOutcome::Committed);
            }
            "stop" => {
                let owner = source.owner.clone();
                f.state
                    .collab(move |tx| wakes::human_stop(tx, &owner))
                    .await
                    .unwrap();
            }
            "restore" => {
                f.state
                    .collab(|tx| {
                        crewd_store::judgment_state::restored(tx.conn())?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
            "admission" => {
                f.state.admission.revoke(&source.owner);
            }
            _ => unreachable!(),
        }
        task.await.unwrap();
        let v = view(&f, &source.id).await;
        assert_eq!(v["decisionRecord"]["adopted"], false, "{mutation}: {v}");
        assert_ne!(v["decisionRecord"]["phase"], "answered", "{mutation}: {v}");
        assert_eq!(v["execution"], "not_started");
        assert_eq!(fake.requests().len(), 1);
        if mutation == "admission" {
            assert_eq!(v["decisionRecord"]["phase"], "unknown");
            f.state.admission.allow(&source.owner);
            f.runtime
                .run_admin_case(f.crew.clone(), source.id.clone(), true)
                .await
                .unwrap();
            assert_eq!(
                fake.requests().len(),
                1,
                "consumed unknown call cannot become retryable after readmission"
            );
        }
        f.close().await;
    }
}
#[tokio::test]
async fn dispatched_database_reopen_becomes_unknown_and_is_not_sent_again() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let mut f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = sources(&f, 1).await.remove(0);
    let crew = f.crew.clone();
    let id = source.id.clone();
    f.state
        .collab(move |tx| {
            let s = cases::prepare(tx, &crew, &id)?.unwrap();
            assert!(cases::dispatch(tx, &s)?);
            Ok(())
        })
        .await
        .unwrap();
    assert!(fake.requests().is_empty());
    f.reopen().await;
    f.runtime.recover_admin_cases().await.unwrap();
    f.state.admission.allow(&source.owner);
    f.runtime.resume_pending_admin_cases().await.unwrap();
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let v = view(&f, &source.id).await;
    assert_eq!(v["decisionRecord"]["phase"], "unknown", "{v}");
    assert_eq!(v["decisionRecord"]["adopted"], false);
    assert!(fake.requests().is_empty());
    f.close().await;
}
#[tokio::test]
async fn source_scan_crosses_sixty_four_without_starving_or_repeating_provider_calls() {
    let fake = FakeJev::start((0..65).map(|_| answer(0.99)).collect()).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let sources = sources(&f, 65).await;
    for s in &sources {
        f.state.admission.allow(&s.owner);
    }
    tokio::time::timeout(LIMIT, async {
        loop {
            f.runtime.resume_pending_admin_cases().await.unwrap();
            let mut complete = 0;
            for s in &sources {
                if view(&f, &s.id).await["decisionRecord"]["phase"] == "answered" {
                    complete += 1;
                }
            }
            if complete == 65 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fake.requests().len(), 65);
    f.runtime.resume_pending_admin_cases().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fake.requests().len(), 65);
    assert_eq!(counts(&f).await.0, 0);
    f.close().await;
}

#[tokio::test]
async fn first_explicit_configuration_can_judge_preexisting_unconfigured_case() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let before = view(&f, &source.id).await;
    assert_eq!(before["decisionRecord"]["reason"], "unconfigured");
    assert!(fake.requests().is_empty());
    f.configure(&fake.base_url).await;
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let after = answered_view(&f, &source.id).await;
    assert_eq!(
        after["decisionRecord"]["phase"], "answered",
        "first legal provider configuration must not strand original case: before={before}; after={after}"
    );
    assert_eq!(after["decisionRecord"]["adopted"], true);
    assert_eq!(fake.requests().len(), 1);
    assert_eq!(after["execution"], "not_started");
    f.close().await;
}

#[tokio::test]
async fn provider_receives_only_scoped_summary_without_foreign_text_or_private_identifiers() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = source_fixture(&f, 1, true).await.remove(0);
    let original = view(&f, &source.id).await;
    assert_eq!(original["targets"].as_array().unwrap().len(), 2);
    assert!(
        original["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["scope"] == "out_of_scope")
    );
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    answered_view(&f, &source.id).await;
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(
        !body.contains("FOREIGN_PRIVATE_ADMIN_SOURCE_SENTINEL"),
        "foreign source text leaked: {body}"
    );
    for private in [&source.id, &source.owner, &f.crew] {
        assert!(
            !body.contains(private),
            "private identity {private} leaked: {body}"
        );
    }
    for target in original["targets"].as_array().unwrap() {
        for field in ["id", "crewId"] {
            let raw = serde_json::to_string(target[field].as_str().unwrap()).unwrap();
            assert!(
                !body.contains(&raw),
                "original target identity leaked: {body}"
            );
        }
    }
    assert!(
        body.len() < 64 * 1024,
        "model request exceeded bounded evidence budget"
    );
    assert_eq!(view(&f, &source.id).await["execution"], "not_started");
    f.close().await;
}

#[tokio::test]
async fn periodic_runtime_pump_observes_durable_source_without_direct_dispatch() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    // The helper commits without hub publication. Only the running production tick
    // can find this source; do not call run_admin_case or resume_pending_admin_cases.
    let source = sources(&f, 1).await.remove(0);
    let before = counts(&f).await;
    let settled = tokio::time::timeout(LIMIT, async {
        loop {
            let result = view(&f, &source.id).await;
            if result["decisionRecord"]["phase"] == "answered" {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled["decisionRecord"]["adopted"], true);
    assert_eq!(settled["execution"], "not_started");
    assert_eq!(fake.requests().len(), 1);
    assert_eq!(counts(&f).await, before);
    f.close().await;
}
