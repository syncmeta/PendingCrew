//! Actual production actor/render/claim/delivery over the organization authority.
//! RelayStandIn is the only executor; no provider or app is launched.
#![cfg(feature = "testkit")]
use crewd_collab::{Sender, asks, wakes, work};
use crewd_exec::{
    delivery::Subject,
    session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
};
use crewd_proto::{Role, WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
use crewd_server::{
    agent::testkit::{CallerStage, Fixture, RelayStandIn},
    orchestra::{
        self, Actor, Handle, OrchestraConfig,
        clock::ManualClock,
        core::{Origin, StartSession},
        db::{DbPort, pump_collab},
        exec::Executors,
        hooks::Hooks,
    },
};
use crewd_store::object_refs::ObjectKind;
use crewd_wake::Ms;
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{task::JoinHandle, time::timeout};
const LIMIT: Duration = Duration::from_secs(10);
struct Rig {
    f: Fixture,
    h: Handle,
    fake: RelayStandIn,
    actor: Option<Actor>,
    forward: JoinHandle<()>,
    owner: String,
}
impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(a) = &self.actor {
            a.abort_handle().abort();
        }
        self.forward.abort();
    }
}
impl Rig {
    async fn new() -> Self {
        let cell = Arc::new(OnceLock::new());
        let f = Fixture::with_orchestra(cell.clone()).await;
        f.state
            .db
            .call(|s| {
                let tx = s.conn().unchecked_transaction()?;
                crewd_store::work_organization::migrate_in_tx(&tx, "2026-10-13T00:00:00.000Z")?;
                tx.commit()?;
                Ok::<_, crewd_store::StoreError>(())
            })
            .await
            .unwrap()
            .unwrap();
        let rx = f.state.hub.subscribe_collab();
        let fake = RelayStandIn::default();
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
            Executors::new().with("claude", Arc::new(fake.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(Ms(crewd_store::now_ms() + 60_000))),
            OrchestraConfig {
                stagger_ms: 0,
                heartbeat_ms: 10,
                retry_jitter: 0.0,
                jitter_seed: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(cell.set(h.clone()).is_ok());
        assert!(f.state.orchestra.set(h.clone()).is_ok());
        let sink = h.clone();
        let forward = tokio::spawn(pump_collab(rx, move |fact| sink.collab(fact)));
        let owner = h
            .start(
                StartSession {
                    codex_options: None,

                    crew_id: f.crew.clone(),
                    captain: false,
                    executor: "claude".into(),
                    brief: "synthetic actor acceptance".into(),
                    title: Some("actor owner".into()),
                    model: None,
                    effort: None,
                    isolation: false,
                },
                Origin::Human,
            )
            .await
            .unwrap()
            .id;
        timeout(LIMIT, async {
            while !h.live(&owner).await.unwrap().running {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        fake.emit(
            &owner,
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        let r = Self {
            f,
            h,
            fake,
            actor: Some(actor),
            forward,
            owner,
        };
        r.working(true).await;
        r
    }
    async fn working(&self, on: bool) {
        self.fake.emit(&self.owner, SessionEvent::Working(on));
        timeout(LIMIT, async {
            while self.h.live(&self.owner).await.unwrap().working != on {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn rows(&self) -> Vec<wakes::WakeRow> {
        let owner = self.owner.clone();
        self.f
            .state
            .db
            .call_result(move |s| wakes::list(s.conn(), Some(&owner), 100))
            .await
            .unwrap()
            .unwrap()
    }
    async fn settle(&self, id: i64, state: wakes::WakeState) -> wakes::WakeRow {
        timeout(LIMIT, async {
            loop {
                let row = self.rows().await.into_iter().find(|r| r.id == id).unwrap();
                if row.state == state {
                    return row;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(mut self) {
        assert!(
            timeout(LIMIT, self.h.shutdown(Duration::from_secs(2)))
                .await
                .unwrap()
        );
        timeout(LIMIT, self.actor.take().unwrap().finished())
            .await
            .unwrap()
            .unwrap();
        self.forward.abort();
        let _ = (&mut self.forward).await;
        let db = self.f.state.db.clone();
        timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
            .await
            .unwrap()
            .unwrap();
    }
}
struct Scenario {
    plans: Vec<crewd_proto::WorkPlan>,
    child: crewd_proto::WorkPlan,
    ask: Option<String>,
    crew: String,
}
async fn scenario(r: &Rig, human: bool) -> Scenario {
    let b = r.f.add_crew("actor B", None).await;
    r.f.add_session(&b, "actor-b-owner", Role::Worker, "B owner")
        .await;
    let crew = r.f.crew.clone();
    let owner = r.owner.clone();
    let child =
        r.f.state
            .collab(move |tx| {
                work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: b,
                        title: "cross crew prerequisite".into(),
                        owner: Some("actor-b-owner".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
    let childid = child.id.clone();
    let c = crew.clone();
    let plans =
        r.f.state
            .collab(move |tx| {
                let mut plans = Vec::new();
                for title in ["actor A one", "actor A two"] {
                    plans.push(work::create(
                        tx,
                        &Sender::Human,
                        WorkCreateParams {
                            crew_id: c.clone(),
                            title: title.into(),
                            owner: Some(owner.clone()),
                            depends_on: vec![childid.clone()],
                            ..Default::default()
                        },
                    )?);
                }
                Ok(plans)
            })
            .await
            .unwrap();
    let ask = if human {
        let ps = plans.clone();
        let c = crew.clone();
        let own = r.owner.clone();
        let bn = child.id[1..].parse::<i64>().unwrap();
        Some(
            r.f.state
                .collab(move |tx| {
                    let b = crewd_store::work_organization::core_for_reference(
                        tx.conn(),
                        ObjectKind::Plan,
                        bn,
                    )?
                    .unwrap()
                    .parse()
                    .unwrap();
                    let mut ids = Vec::new();
                    for p in &ps {
                        ids.push(
                            crewd_store::work_organization::core_for_reference(
                                tx.conn(),
                                ObjectKind::Plan,
                                p.id[1..].parse().unwrap(),
                            )?
                            .unwrap()
                            .parse()
                            .unwrap(),
                        );
                    }
                    let h = asks::with_book(tx, &c, |book, now| {
                        let first = book
                            .block(
                                &crewd_work::Actor::worker(&own),
                                now,
                                ids[0],
                                vec![
                                    crewd_work::BlockOn::Item(b),
                                    crewd_work::BlockOn::NewAsk {
                                        question: "actual actor final choice".into(),
                                        default: None,
                                    },
                                ],
                                crewd_work::Continuation::Nothing,
                            )?
                            .value
                            .asks[0];
                        book.block(
                            &crewd_work::Actor::worker(&own),
                            now,
                            ids[1],
                            vec![
                                crewd_work::BlockOn::Item(b),
                                crewd_work::BlockOn::Ask(first),
                            ],
                            crewd_work::Continuation::Nothing,
                        )?;
                        Ok(first)
                    })?;
                    Ok(crewd_store::work_organization::reference_for_core(
                        tx.conn(),
                        ObjectKind::HumanTask,
                        &h.to_string(),
                    )?
                    .unwrap()
                    .id())
                })
                .await
                .unwrap(),
        )
    } else {
        None
    };
    Scenario {
        plans,
        child,
        ask,
        crew,
    }
}
async fn finish_child(r: &Rig, s: &Scenario) {
    let id = s.child.id.clone();
    let rev = s.child.revision.clone();
    r.f.state
        .collab(move |tx| {
            work::update(
                tx,
                &Sender::Session("actor-b-owner".into()),
                WorkUpdateParams {
                    id,
                    expected_revision: rev,
                    status: Some(WorkUpdateStatus::Done),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn organization_last_human_blocker_is_actually_delivered_once_to_current_owner() {
    let r = Rig::new().await;
    let s = scenario(&r, true).await;
    finish_child(&r, &s).await;
    for p in &s.plans {
        let id = p.id.clone();
        let v =
            r.f.state
                .db
                .call_result(move |st| work::get(st.conn(), &id))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(v.status, "blocked");
        assert_eq!(v.blocked_on, vec![s.ask.clone().unwrap()]);
    }
    let crew = s.crew.clone();
    let ask = s.ask.clone().unwrap();
    r.f.state
        .collab(move |tx| asks::answer(tx, &crew, &ask, "approved format"))
        .await
        .unwrap();
    let rows = r.rows().await;
    let wake: Vec<_> = rows
        .iter()
        .filter(|q| q.kind == wakes::WakeKind::AskAnswer)
        .collect();
    assert_eq!(
        wake.len(),
        1,
        "one owner should receive one merged notification"
    );
    let id = wake[0].id;
    r.working(false).await;
    timeout(LIMIT, async {
        while r.fake.sent(&r.owner).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let texts = r.fake.sent(&r.owner);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains(s.ask.as_ref().unwrap()));
    for p in &s.plans {
        assert!(texts[0].contains(&p.id));
        let pid = p.id.clone();
        let current =
            r.f.state
                .db
                .call_result(move |st| work::get(st.conn(), &pid))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(current.owner.as_deref(), Some(r.owner.as_str()));
        assert!(current.revision.parse::<u64>().unwrap() > p.revision.parse::<u64>().unwrap());
    }
    let inflight = r.settle(id, wakes::WakeState::Inflight).await;
    assert_eq!(inflight.attempts, 1);
    r.fake.emit(
        &r.owner,
        SessionEvent::Delivery {
            id: r.fake.last_send_id(&r.owner),
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    r.settle(id, wakes::WakeState::Delivered).await;
    assert_eq!(r.fake.sent(&r.owner).len(), 1);
    assert_eq!(r.fake.launches(&r.owner).len(), 1);
    r.close().await;
}
#[tokio::test]
async fn organization_completion_after_actual_render_is_dropped_before_executor_claim() {
    let r = Rig::new().await;
    let s = scenario(&r, false).await;
    finish_child(&r, &s).await;
    let rows = r.rows().await;
    let wake: Vec<_> = rows
        .iter()
        .filter(|q| q.source.starts_with("work-done:"))
        .collect();
    assert_eq!(wake.len(), 1);
    let id = wake[0].id;
    let mut gate = r.f.caller_gate(&r.owner, CallerStage::WakeRendered);
    r.fake.emit(&r.owner, SessionEvent::Working(false));
    let rendered = timeout(LIMIT, gate.entered()).await.unwrap();
    for p in &s.plans {
        assert!(rendered.text.contains(&p.id));
    }
    for p in &s.plans {
        let pid = p.id.clone();
        let owner = r.owner.clone();
        r.f.state
            .collab(move |tx| {
                let current = work::get(tx.conn(), &pid)?;
                work::update(
                    tx,
                    &Sender::Session(owner),
                    WorkUpdateParams {
                        id: pid,
                        expected_revision: current.revision,
                        status: Some(WorkUpdateStatus::Done),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
    }
    gate.release();
    let dropped = r.settle(id, wakes::WakeState::Dropped).await;
    assert_eq!(dropped.attempts, 0);
    r.h.latches().await.unwrap();
    assert!(r.fake.sent(&r.owner).is_empty());
    assert_eq!(r.fake.launches(&r.owner).len(), 1);
    r.close().await;
}
