//! Real old-format announcements are historical evidence, never historical grants.
use super::*;

#[tokio::test]
async fn corrupt_typed_source_blocks_index_authority_but_does_not_hide_later_history() {
    let mut f = Fixture::new_with_admin_dispatch(true).await;
    let _modern = sources(&f, 1).await;
    let crew = f.crew.clone();
    let other = f
        .state
        .collab(move |tx| {
            sessions::register(
                tx,
                sessions::NewSession {
                    crew_id: crew,
                    role: Role::Worker,
                    executor: "claude".into(),
                    title: Some("legacy owner".into()),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .id;
    f.worker = other.clone();
    f.state.admission.allow(&other);
    let old = old_announcement(&f, 1, crewd_proto::SenderKind::System).await;
    scanned(&f, &old).await;
    assert_eq!(preview(&f, &old).await["canConfirm"], true);
    f.runtime.shutdown().await;
    let damaged = "{damaged typed source retained".to_string();
    let injected = damaged.clone();
    let key = f
        .state
        .db
        .call_result(move |s| {
            let key: String = s.conn().query_row(
                "SELECT key FROM meta WHERE key LIKE 'admin.case.v1:%' LIMIT 1",
                [],
                |r| r.get(0),
            )?;
            crewd_store::meta_set(s.conn(), &key, &injected)?;
            s.with_tx(|tx| crewd_store::judgment_state::restored(tx))?;
            Ok::<_, crewd_store::StoreError>(key)
        })
        .await
        .unwrap()
        .unwrap();
    f.state
        .collab(crewd_collab::admin_case_legacy::tick)
        .await
        .unwrap();
    let list = f
        .rpc("adminCase.legacyList", json!({"limit":100}))
        .await
        .unwrap();
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["messageId"] == old.message),
        "bad typed source cannot starve later historical discovery: {list}"
    );
    assert!(
        list["scan"]["unverifiedSources"].as_u64().unwrap() > 0,
        "{list}"
    );
    let p = preview(&f, &old).await;
    assert_eq!(p["canConfirm"], false);
    assert!(
        p["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "index_incomplete")
    );
    let before = metadata(&f).await;
    assert!(
        f.rpc(
            "adminCase.legacyConfirm",
            confirmation(&f, &old, &p, &uuid::Uuid::new_v4().to_string())
        )
        .await
        .is_err()
    );
    assert_eq!(metadata(&f).await, before);
    let raw = f
        .state
        .db
        .call_result(move |s| crewd_store::meta_get(s.conn(), &key))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw, Some(damaged));
    f.close().await;
}

#[tokio::test]
async fn discovered_legacy_row_damage_or_removal_cannot_become_an_empty_list_or_new_grant() {
    for missing in [false, true] {
        let f = Fixture::new_with_admin_dispatch(true).await;
        let old = old_announcement(&f, 1, crewd_proto::SenderKind::System).await;
        let observed = scanned(&f, &old).await;
        f.runtime.shutdown().await;
        let p = preview(&f, &old).await;
        assert_eq!(p["canConfirm"], true);
        let key = format!(
            "admin.legacy.row.v1:{:020}",
            observed["messageSeq"].as_i64().unwrap()
        );
        let k = key.clone();
        f.state
            .db
            .call_result(move |s| {
                if missing {
                    s.conn().execute("DELETE FROM meta WHERE key=?1", [k])?;
                } else {
                    crewd_store::meta_set(s.conn(), &k, "{damaged legacy envelope retained")?;
                }
                Ok::<_, crewd_store::StoreError>(())
            })
            .await
            .unwrap()
            .unwrap();
        let listed = f.rpc("adminCase.legacyList", json!({"limit":100})).await;
        if let Ok(list) = listed {
            assert!(
                list["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["messageId"] == old.message && v["state"] == "invalid"),
                "missing={missing}: damaged discovered row disappeared: {list}"
            );
        }
        let before = metadata(&f).await;
        assert!(
            f.rpc(
                "adminCase.legacyConfirm",
                confirmation(&f, &old, &p, &uuid::Uuid::new_v4().to_string())
            )
            .await
            .is_err(),
            "missing={missing}: lost discovery evidence became a new grant"
        );
        assert_eq!(metadata(&f).await, before);
        f.state
            .collab(crewd_collab::admin_case_legacy::tick)
            .await
            .unwrap();
        let raw = f
            .state
            .db
            .call_result(move |s| crewd_store::meta_get(s.conn(), &key))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            raw,
            if missing {
                None
            } else {
                Some("{damaged legacy envelope retained".into())
            }
        );
        f.close().await;
    }
}

pub(super) struct Legacy {
    pub(super) message: String,
    pub(super) works: Vec<String>,
    pub(super) owner: String,
}

pub(super) async fn old_announcement(
    f: &Fixture,
    count: usize,
    sender: crewd_proto::SenderKind,
) -> Legacy {
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    f.state
        .db
        .call_result(move |store| {
            if crews::management(store.conn())?.is_none() {
                crewd_store::crews::upsert(
                    store.conn(),
                    &crewd_proto::Crew {
                        id: "legacy-management".into(),
                        title: "legacy management".into(),
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
                sessions::set_desired(tx, &owner, Desired::Running)?;
                let mut works = vec![];
                for n in 0..count {
                    works.push(
                        work::create(
                            tx,
                            &Sender::Human,
                            WorkCreateParams {
                                crew_id: crew.clone(),
                                owner: Some(owner.clone()),
                                title: format!("legacy unresolved {n}"),
                                ..Default::default()
                            },
                        )?
                        .id,
                    );
                }
                let budget = crewd_work::StopStreak {
                    escalated: true,
                    rewakes: 3,
                    unimproved: 3,
                    last: works
                        .iter()
                        .map(|id| crewd_work::StopViolation {
                            item: id.parse().unwrap(),
                            kind: crewd_work::StopKind::ReadyUnscheduled,
                        })
                        .collect(),
                };
                // This is the actual migrated epoch-zero shape, not a modern case
                // whose audit fields have been deleted to make it appear old.
                tx.conn().execute(
                    "INSERT INTO work_stop_checks(session_id,epoch,state) VALUES(?1,0,?2)",
                    rusqlite::params![owner, serde_json::to_string(&budget).unwrap()],
                )?;
                let home = crews::management(tx.conn())?.unwrap();
                let channel = crewd_collab::channels::ensure_human_channel(tx, &home)?;
                let summary = json!({"stopWork":works.iter().take(50).map(|id|
                json!({"id":id,"kind":"ready_unscheduled"})).collect::<Vec<_>>(),
                "total":works.len(),"partial":works.len()>50});
                let message = uuid::Uuid::new_v4().to_string();
                crewd_store::messages::insert(
                    tx.conn(),
                    &crewd_proto::Message {
                        id: message.clone(),
                        channel_id: channel.id,
                        sender_kind: sender,
                        text: format!("Stop-check escalation for {owner}: {summary}"),
                        category: Some("work_stop_escalation".into()),
                        mentions: vec![crewd_proto::Mention {
                            kind: crewd_proto::MentionKind::Human,
                            target_id: None,
                        }],
                        created_at: tx.now_ts().into(),
                        ..Default::default()
                    },
                    None,
                )?;
                Ok(Legacy {
                    message,
                    works,
                    owner,
                })
            })
            .map(|(v, _)| v)
        })
        .await
        .unwrap()
        .unwrap()
}

async fn preview(f: &Fixture, old: &Legacy) -> serde_json::Value {
    f.rpc(
        "adminCase.legacyPreview",
        json!({"messageId":old.message,
        "crewId":f.crew,"sessionId":old.owner}),
    )
    .await
    .unwrap()
}

async fn scanned(f: &Fixture, old: &Legacy) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(14), async {
        loop {
            if let Ok(v) = f.rpc("adminCase.legacyList", json!({"limit":50})).await {
                if let Some(found) = v["items"]
                    .as_array()
                    .and_then(|items| items.iter().find(|item| item["messageId"] == old.message))
                {
                    return found.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("production periodic scanner must discover real old announcement")
}

fn confirmation(f: &Fixture, old: &Legacy, p: &serde_json::Value, id: &str) -> serde_json::Value {
    json!({"messageId":old.message,"crewId":f.crew,"sessionId":old.owner,
        "requestId":id,"expectedPreviewRevision":p["revision"],
        "note":"Human checked all current targets; old summary is not historical authority"})
}

async fn wait_phase(f: &Fixture, id: &str, field: &str, phase: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(18), async {
        loop {
            let v = view(f, id).await;
            if v[field]["phase"] == phase {
                break v;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {field}.{phase}"))
}

async fn metadata(f: &Fixture) -> Vec<(String, String)> {
    f.state
        .db
        .call_result(|s| {
            let mut q = s
                .conn()
                .prepare("SELECT key,value FROM meta ORDER BY key")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn legacy_preview_reads_are_pure_and_failed_confirmation_rolls_back_real_creation() {
    let f = Fixture::new_with_admin_dispatch(true).await;
    let old = old_announcement(&f, 2, crewd_proto::SenderKind::System).await;
    scanned(&f, &old).await;
    f.runtime.shutdown().await;
    let before = metadata(&f).await;
    let p = preview(&f, &old).await;
    assert_eq!(p["canConfirm"], true, "positive current precondition: {p}");
    f.rpc("adminCase.legacyGet", json!({"messageId":old.message}))
        .await
        .unwrap();
    f.rpc("adminCase.legacyList", json!({"limit":10}))
        .await
        .unwrap();
    assert_eq!(metadata(&f).await, before);
    let args: crewd_proto::AdminCaseLegacyConfirmParams = serde_json::from_value(confirmation(
        &f,
        &old,
        &p,
        &uuid::Uuid::new_v4().to_string(),
    ))
    .unwrap();
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered = reached.clone();
    let refused = f
        .state
        .collab(move |tx| -> crewd_collab::Result<()> {
            crewd_collab::admin_case_legacy::confirm(tx, &Sender::Human, args)?;
            entered.store(true, Ordering::SeqCst);
            Err(
                crewd_store::StoreError::Invalid("injected failure after real recovery".into())
                    .into(),
            )
        })
        .await;
    assert!(
        reached.load(Ordering::SeqCst),
        "failure must happen after real source creation"
    );
    assert!(refused.is_err());
    assert_eq!(metadata(&f).await, before);
    f.close().await;
}

#[tokio::test]
async fn duplicate_text_is_not_duplicate_identity_and_textual_sender_is_not_authority() {
    let f = Fixture::new_with_admin_dispatch(true).await;
    let old = old_announcement(&f, 1, crewd_proto::SenderKind::System).await;
    let id = old.message.clone();
    let (duplicate,forged)=f.state.db.call_result(move |s| {
        let mut m=crewd_store::messages::get(s.conn(),&id)?.unwrap();
        m.text="Stop-check escalation for nonexistent-privileged-source: untrusted historical text".into();
        s.conn().execute("UPDATE messages SET text=?1 WHERE id=?2",rusqlite::params![m.text,id])?;
        m.id=uuid::Uuid::new_v4().to_string();
        crewd_store::messages::insert(s.conn(),&m,None)?;
        let duplicate=m.id.clone();
        m.id=uuid::Uuid::new_v4().to_string();m.sender_kind=crewd_proto::SenderKind::Human;
        crewd_store::messages::insert(s.conn(),&m,None)?;
        Ok::<_,crewd_store::StoreError>((duplicate,m.id))
    }).await.unwrap().unwrap();
    scanned(&f, &old).await;
    f.runtime.shutdown().await;
    let a = f
        .rpc("adminCase.legacyGet", json!({"messageId":old.message}))
        .await
        .unwrap();
    let b = f
        .rpc("adminCase.legacyGet", json!({"messageId":duplicate}))
        .await
        .unwrap();
    assert_ne!(a["messageId"], b["messageId"]);
    assert_eq!(a["historicalText"], b["historicalText"]);
    assert!(a["linkedCaseId"].is_null() && b["linkedCaseId"].is_null());
    let p = preview(&f, &old).await;
    assert_eq!(
        p["canConfirm"], true,
        "Human explicitly selects the actual current member"
    );
    assert_eq!(p["sessionId"], old.owner);
    let forged_preview = f
        .rpc(
            "adminCase.legacyPreview",
            json!({"messageId":forged,"crewId":f.crew,"sessionId":old.owner}),
        )
        .await
        .unwrap();
    assert_eq!(forged_preview["canConfirm"], false);
    let confirmed = f
        .rpc(
            "adminCase.legacyConfirm",
            confirmation(&f, &old, &p, &uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap();
    let current = view(&f, confirmed["caseId"].as_str().unwrap()).await;
    assert_eq!(current["source"]["sessionId"], old.owner);
    assert_eq!(
        current["source"]["provenance"]["kind"],
        "human_revalidation"
    );
    let unchanged = f
        .rpc("adminCase.legacyGet", json!({"messageId":duplicate}))
        .await
        .unwrap();
    assert!(
        unchanged["linkedCaseId"].is_null(),
        "equal text cannot bind another message implicitly"
    );
    f.close().await;
}

#[tokio::test]
async fn modern_typed_announcement_is_linked_without_a_second_recovery_case() {
    let f = Fixture::new_with_admin_dispatch(true).await;
    let source = sources(&f, 1).await.remove(0);
    let case = view(&f, &source.id).await;
    let message = case["source"]["announcementId"]
        .as_str()
        .unwrap()
        .to_owned();
    let old = Legacy {
        message,
        works: vec![source.work],
        owner: source.owner,
    };
    let indexed = scanned(&f, &old).await;
    assert_eq!(indexed["linkedCaseId"], source.id);
    let p = preview(&f, &old).await;
    assert_eq!(p["existingCaseId"], source.id);
    let before = view(&f, &source.id).await["source"].clone();
    let result = f
        .rpc(
            "adminCase.legacyConfirm",
            confirmation(&f, &old, &p, &uuid::Uuid::new_v4().to_string()),
        )
        .await;
    if let Ok(receipt) = result {
        assert_eq!(receipt["caseId"], source.id);
        assert_eq!(receipt["created"], false);
    }
    assert_eq!(view(&f, &source.id).await["source"], before);
    let list = f
        .rpc("adminCase.list", json!({"crewId":f.crew,"limit":100}))
        .await
        .unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn current_cross_crew_target_outside_old_summary_is_retained_without_granting_access() {
    let f = Fixture::new_with_admin_dispatch(true).await;
    let old = old_announcement(&f, 55, crewd_proto::SenderKind::System).await;
    let owner = old.owner.clone();
    let foreign = f
        .state
        .collab(move |tx| {
            let crew = crews::create(
                tx,
                crews::NewCrew {
                    title: "foreign target crew".into(),
                    working_directory: "/tmp/foreign-legacy-target".into(),
                    ..Default::default()
                },
            )?;
            let w = work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.id,
                    owner: Some(owner.clone()),
                    title: "not in old fifty-five".into(),
                    ..Default::default()
                },
            )?;
            let mut b = crewd_store::work_stop_checks::get(tx.conn(), &owner)?.unwrap();
            b.streak.last.push(crewd_work::StopViolation {
                item: w.id.parse().unwrap(),
                kind: crewd_work::StopKind::ReadyUnscheduled,
            });
            assert!(crewd_store::work_stop_checks::save(
                tx.conn(),
                &owner,
                Some(b.epoch),
                &b.streak
            )?);
            Ok(w.id)
        })
        .await
        .unwrap();
    scanned(&f, &old).await;
    let p = preview(&f, &old).await;
    assert_eq!(p["canConfirm"], true, "{p}");
    let targets = p["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 56);
    assert_eq!(
        targets.iter().find(|v| v["id"] == foreign).unwrap()["scope"],
        "out_of_scope"
    );
    let result = f
        .rpc(
            "adminCase.legacyConfirm",
            confirmation(&f, &old, &p, &uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap();
    let case = view(&f, result["caseId"].as_str().unwrap()).await;
    assert_eq!(case["targets"], p["targets"]);
    assert_ne!(case["outcomeRecord"]["phase"], "resolved");
    f.close().await;
}

#[tokio::test]
async fn current_preview_cas_rejects_work_stop_configuration_restore_and_source_crew_changes() {
    for change in ["work", "stop", "config", "restore", "crew"] {
        let f = Fixture::new_with_admin_dispatch(true).await;
        let old = old_announcement(&f, 1, crewd_proto::SenderKind::System).await;
        scanned(&f, &old).await;
        f.runtime.shutdown().await;
        let p = preview(&f, &old).await;
        assert_eq!(p["canConfirm"], true, "positive current precondition: {p}");
        let args = confirmation(&f, &old, &p, &uuid::Uuid::new_v4().to_string());
        match change {
            "work" => {
                let plan = current_work(&f, &old.works[0]).await;
                // Same current Work mutation through Human API, without an Admin grant.
                f.rpc("work.close",json!({"id":plan.id,"expectedRevision":plan.revision,
                    "kind":"done","evidence":{"kind":"text","value":"independent completion"},"note":"completed after preview"})).await.unwrap();
            }
            "stop" => {
                let owner = old.owner.clone();
                f.state
                    .collab(move |tx| wakes::human_stop(tx, &owner))
                    .await
                    .unwrap();
            }
            "config" => {
                let fake = FakeJev::start(vec![]).await;
                f.configure(&fake.base_url).await;
            }
            "restore" => {
                f.state
                    .db
                    .call_result(|s| s.with_tx(|c| crewd_store::judgment_state::restored(c)))
                    .await
                    .unwrap()
                    .unwrap();
            }
            "crew" => {
                let owner = old.owner.clone();
                f.state
                    .collab(move |tx| {
                        let crew = crews::create(
                            tx,
                            crews::NewCrew {
                                title: "different crew".into(),
                                working_directory: "/tmp/different-fixture".into(),
                                ..Default::default()
                            },
                        )?;
                        let mut s = sessions::get(tx.conn(), &owner)?;
                        s.crew_id = crew.id;
                        crewd_store::sessions::upsert(tx.conn(), &s, None)?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = metadata(&f).await;
        assert!(
            f.rpc("adminCase.legacyConfirm", args).await.is_err(),
            "{change}"
        );
        assert_eq!(
            metadata(&f).await,
            before,
            "{change}: stale confirmation wrote authority"
        );
        f.close().await;
    }
}

#[tokio::test]
async fn legacy_fifty_five_human_revalidation_runs_production_judgment_admin_and_typed_effects() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new_with_admin_dispatch(true).await;
    f.configure(&fake.base_url).await;
    let r = Rig::new(f).await;
    let old = old_announcement(&r.f, 55, crewd_proto::SenderKind::System).await;
    r.handle
        .resume(&old.owner, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.exec.launches(&old.owner).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.exec
        .emit(&old.owner, crewd_exec::session::SessionEvent::Working(true));
    let historical = scanned(&r.f, &old).await;
    assert_eq!(historical["historyComplete"], false);
    assert!(historical["linkedCaseId"].is_null());
    assert!(fake.requests().is_empty(), "scan is not a new grant");
    let p = preview(&r.f, &old).await;
    assert_eq!(p["canConfirm"], true, "{p}");
    assert_eq!(p["targets"].as_array().unwrap().len(), 55);
    assert_eq!(p["budgetEpoch"], 0);
    let request_id = uuid::Uuid::new_v4().to_string();
    let args = confirmation(&r.f, &old, &p, &request_id);
    let (a, b) = tokio::join!(
        r.f.rpc("adminCase.legacyConfirm", args.clone()),
        r.f.rpc("adminCase.legacyConfirm", args.clone())
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a["caseId"], b["caseId"]);
    let id = a["caseId"].as_str().unwrap().to_owned();
    let receipt =
        r.f.rpc("adminCase.legacyReceipt", json!({"requestId":request_id}))
            .await
            .unwrap();
    assert_eq!(receipt["caseId"], id);
    let running = wait_phase(&r.f, &id, "executionRecord", "running").await;
    assert_eq!(
        running["source"]["provenance"]["kind"],
        "human_revalidation"
    );
    assert_eq!(
        running["source"]["provenance"]["legacyMessageId"],
        old.message
    );
    assert_eq!(
        running["source"]["provenance"]["previewRevision"],
        p["revision"]
    );
    assert!(
        !running["source"]["provenance"]["announcementDigest"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    let sid = running["executionRecord"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(r.exec.launches(&sid).len(), 1);
    acknowledge_initial_input(&r, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    for id in &old.works {
        let plan = current_work(&r.f, id).await;
        let result = socket.call("agent.call", close_args(&plan)).await.unwrap();
        assert_eq!(result["isError"], false, "{id}: {result}");
    }
    let resolved = wait_phase(&r.f, &id, "outcomeRecord", "resolved").await;
    assert_eq!(
        resolved["outcomeRecord"]["resolvedByAdmin"]
            .as_array()
            .unwrap()
            .len(),
        55
    );
    assert_eq!(fake.requests().len(), 1);
    let owner = old.owner.clone();
    let budget =
        r.f.state
            .db
            .call_result(move |s| crewd_store::work_stop_checks::get(s.conn(), &owner))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(
        budget.epoch, 0,
        "recovery must not reset or manufacture stop-check rounds"
    );
    socket.close().await;
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
    let reopened = Rig::new(f).await;
    let lookup = reopened
        .f
        .rpc("adminCase.legacyReceipt", json!({"requestId":request_id}))
        .await
        .unwrap();
    assert_eq!(
        lookup["caseId"], id,
        "unknown response recovery reads the original UUID receipt"
    );
    let after = view(&reopened.f, &id).await;
    assert_eq!(after["executionRecord"]["phase"], "unknown");
    assert_eq!(after["executionRecord"]["sessionId"], sid);
    reopened.f.runtime.schedule_admin_launches().await.unwrap();
    assert!(reopened.exec.launches(&sid).is_empty());
    let list = reopened
        .f
        .rpc(
            "adminCase.list",
            json!({"crewId":reopened.f.crew,"limit":100}),
        )
        .await
        .unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    reopened.close().await;
}

#[tokio::test]
async fn real_old_fifty_item_summary_requires_human_preview_of_all_fifty_five_current_targets() {
    let f = Fixture::new_with_admin_dispatch(true).await;
    let old = old_announcement(&f, 55, crewd_proto::SenderKind::System).await;
    // Direct scan is only this first backend regression; full-chain acceptance
    // below exercises the actual periodic scanner without invoking tick.
    f.state
        .collab(|tx| crewd_collab::admin_case_legacy::tick(tx))
        .await
        .unwrap();
    let result = f
        .rpc(
            "adminCase.legacyPreview",
            json!({"messageId":old.message,
        "crewId":f.crew,"sessionId":old.owner}),
        )
        .await;
    let budget_owner = old.owner.clone();
    let budget = f
        .state
        .db
        .call_result(move |s| crewd_store::work_stop_checks::get(s.conn(), &budget_owner))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    f.close().await;
    let result =
        result.expect("real historical summary must have a pure current Human review path");
    assert_eq!(result["targets"].as_array().unwrap().len(), 55, "{result}");
    assert_eq!(budget.epoch, 0);
    assert!(budget.streak.escalated);
}
