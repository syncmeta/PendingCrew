//! Shared Port contracts. These are internal real Port/Tx/planner proofs,
//! not generated-helper/actor E2E and not substitutes for the existing three-party caller.
use super::*;
use crate::agent::testkit::Fixture;
use crate::orchestra::{
    db::DbPort,
    port::{Ack, Batch, Decision, Port, Riders},
};
use crewd_collab::{self as collab, causal, messages::NewPost};
use crewd_proto::{Mention, MentionKind, Message};

struct ContractRig {
    mem: Option<MemPort>,
    db: Option<DbPort>,
    fixture: Option<Fixture>,
    cap: String,
    worker: String,
    channels: [String; 2],
}
macro_rules! port_call { ($r:expr,$method:ident($($arg:expr),*))=>{{
    if let Some(p)=&$r.mem {p.$method($($arg),*).await} else {$r.db.as_ref().unwrap().$method($($arg),*).await}
}} }
impl ContractRig {
    async fn new(memory: bool) -> Self {
        let f = Fixture::new().await;
        let crew = f.crew.clone();
        let cap = f.captain.clone();
        let worker = f.worker.clone();
        let channels = [format!("{crew}/s42-one"), format!("{crew}/s42-two")];
        if memory {
            let p = MemPort::new();
            {
                let mut s = p.lock();
                s.sessions.insert(cap.clone(), Rig::row(&cap, &crew, true));
                s.sessions
                    .insert(worker.clone(), Rig::row(&worker, &crew, false));
                for row in s.sessions.values_mut() {
                    row.desired = Desired::Running;
                }
                s.channels.insert(
                    format!("{crew}/human"),
                    MemChannel {
                        crew: crew.clone(),
                        kind: MemKind::Human,
                        owner: None,
                        members: vec![],
                    },
                );
                s.channels.insert(
                    format!("{crew}/lobby"),
                    MemChannel {
                        crew: crew.clone(),
                        kind: MemKind::Lobby,
                        owner: None,
                        members: vec![],
                    },
                );
                for ch in &channels {
                    s.channels.insert(
                        ch.clone(),
                        MemChannel {
                            crew: crew.clone(),
                            kind: MemKind::Other,
                            owner: Some(cap.clone()),
                            members: vec![cap.clone(), worker.clone()],
                        },
                    );
                }
            }
            for ch in &channels {
                p.join(&cap, ch);
                p.join(&worker, ch);
            }
            let db = f.state.db.clone();
            drop(f);
            tokio::task::spawn_blocking(move || db.shutdown())
                .await
                .unwrap();
            Self {
                mem: Some(p),
                db: None,
                fixture: None,
                cap,
                worker,
                channels,
            }
        } else {
            let ids = [cap.clone(), worker.clone()];
            f.state
                .collab(move |tx| {
                    for id in ids {
                        collab::sessions::set_desired(tx, &id, crewd_proto::Desired::Running)?;
                    }
                    Ok(())
                })
                .await
                .unwrap();
            // Real channel membership and cursor pinning, without agent ToolProgress.
            for ch in &channels {
                let ch = ch.clone();
                let c = crew.clone();
                let a = cap.clone();
                let b = worker.clone();
                f.state.collab(move|tx|{
                    tx.conn().execute("INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,?2,'topic',?1,?3,'1970-01-01T00:00:00Z')",rusqlite::params![ch,c,a])?;
                    for id in [&a,&b] {tx.conn().execute("INSERT INTO channel_members(channel_id,member_kind,member_id,joined_at) VALUES(?1,'session',?2,'1970-01-01T00:00:00Z')",rusqlite::params![ch,id])?;collab::cursors::pin_to_end(tx,id,&ch)?;}
                    Ok(())
                }).await.unwrap();
            }
            let p = DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"));
            Self {
                mem: None,
                db: Some(p),
                fixture: Some(f),
                cap,
                worker,
                channels,
            }
        }
    }
    async fn post(&self, ch: usize, sender: collab::Sender, target: &str, text: &str) -> Message {
        let post = NewPost {
            channel_id: self.channels[ch].clone(),
            text: text.into(),
            mentions: vec![Mention {
                kind: MentionKind::Session,
                target_id: Some(target.into()),
            }],
            ..Default::default()
        };
        let message = if let Some(p) = &self.mem {
            p.post_causal(sender, post).unwrap().0
        } else {
            self.fixture
                .as_ref()
                .unwrap()
                .state
                .collab(move |tx| Ok(collab::messages::post(tx, sender, post)?.message))
                .await
                .unwrap()
        };
        self.enqueue_posted(&message).await;
        message
    }
    // Internal shared Port contract: actual committed message facts/context and the
    // production planner determine every target/priority/origin. No actor is claimed.
    async fn enqueue_posted(&self, message: &Message) {
        let at = self.coordinate(message);
        let (facts, _) = port_call!(self, message(&at))
            .unwrap()
            .expect("actual committed message");
        let now = facts.created_at;
        let ctx = port_call!(self, channel_ctx(&at.channel, now))
            .unwrap()
            .expect("actual channel");
        for target in crewd_wake::plan::plan(&facts, &ctx, None, now) {
            let origin = crewd_wake::breaker::origin_of(
                WakeKind::Message,
                Some(&facts.sender),
                &target.session,
            );
            port_call!(
                self,
                enqueue(NewWake {
                    session: target.session,
                    kind: WakeKind::Message,
                    source: crewd_wake::source::message(&facts.id),
                    priority: target.priority,
                    not_before: now,
                    note: None,
                    origin
                })
            )
            .unwrap()
            .expect("new actual planner row");
        }
    }
    fn coordinate(&self, m: &Message) -> crewd_wake::MessageRef {
        if let Some(p) = &self.mem {
            p.causal_local_ref(m).unwrap()
        } else {
            crewd_wake::MessageRef {
                channel: m.channel_id.clone(),
                seq: m.seq,
            }
        }
    }
    async fn batch(&self, target: &str, generation: u64) -> Batch {
        let rows = port_call!(self, pending(target)).unwrap();
        let rendered = port_call!(self, render_wake(target, &rows)).unwrap();
        eprintln!(
            "S42 Port preparation target={target} actual_rows={} actual_targets={:?} body_bytes={}",
            rows.len(),
            rendered.cursor_targets,
            rendered.text.len()
        );
        let mut b = Batch {
            session: target.into(),
            wakes: rows
                .into_iter()
                .filter(|w| !rendered.dropped.contains(&w.id) && !rendered.deferred.contains(&w.id))
                .collect(),
            riders: vec![],
            cursor_targets: rendered.cursor_targets,
            prepared: rendered.prepared,
            lease: None,
        };
        b.lease = match port_call!(self, claim_reserved(&b, generation)).unwrap() {
            Decision::Applied(l) => Some(l),
            other => panic!("actual claim {other:?}"),
        };
        b
    }
    fn fence(&self) -> std::sync::Arc<crate::causal_fence::CausalFence> {
        if let Some(p) = &self.mem {
            p.causal_fence().unwrap()
        } else {
            self.db.as_ref().unwrap().causal_fence().unwrap()
        }
    }
    async fn deliver(&self, b: &mut Batch, id: u64) {
        let cap = b.lease.as_ref().unwrap().causal_claimed().unwrap().clone();
        let g = cap.pending.generation();
        let fence = self.fence();
        fence.binder(&cap.pending, g).unwrap().register(id).unwrap();
        fence.observer(&b.session, g).emitted(id);
        let (_, w) = b.lease.as_mut().unwrap().causal_witness_mut().unwrap();
        *w = Some(Box::new(fence.success(&cap.pending, g, id).unwrap()));
        let a = port_call!(
            self,
            ack(
                b,
                Ack::Delivered {
                    riders: Riders::Release
                }
            )
        )
        .unwrap();
        assert!(!a.stale);
    }
    async fn chain(&self, m: &Message) -> String {
        let cause = if let Some(p) = &self.mem {
            causal::read_message(p.causal.lock().unwrap().store.conn(), m).unwrap()
        } else {
            let m = m.clone();
            self.fixture
                .as_ref()
                .unwrap()
                .state
                .db
                .call(move |s| causal::read_message(s.conn(), &m))
                .await
                .unwrap()
                .unwrap()
        };
        match cause {
            causal::MessageCause::Resolved(f) => {
                assert_eq!(f.components().len(), 1);
                f.components()[0].chain_id()
            }
            _ => panic!("qualified component required"),
        }
    }
    async fn hold(&self, chain: &str, from: &str, target: &str) -> causal::ChainHold {
        let h = causal::ChainHold {
            chain_id: chain.into(),
            from: from.into(),
            target: target.into(),
            hop: 17,
            reason: "budget".into(),
            at_ms: 40,
        };
        if let Some(p) = &self.mem {
            let mut st = p.lock();
            let mut model = p.causal.lock().unwrap();
            model.sync(&st).unwrap();
            let ctx = model.context();
            collab::run_with_context(&mut model.store, 40, ctx, |tx| causal::hold(tx, &h)).unwrap();
            st.now = crewd_wake::Ms(40);
        } else {
            let h = h.clone();
            self.fixture
                .as_ref()
                .unwrap()
                .state
                .collab(move |tx| causal::hold(tx, &h))
                .await
                .unwrap();
        }
        h
    }
    async fn descendant(&self) -> (Message, String) {
        let root = self
            .post(0, collab::Sender::Human, &self.worker, "actual root")
            .await;
        let mut b = self.batch(&self.worker, 1).await;
        self.deliver(&mut b, 51).await;
        let m = self
            .post(
                0,
                collab::Sender::Session(self.worker.clone()),
                &self.cap,
                "actual descendant",
            )
            .await;
        let chain = self.chain(&m).await;
        assert_eq!(chain, self.chain(&root).await);
        (m, chain)
    }
    async fn close(mut self) {
        self.db.take();
        if let Some(f) = self.fixture.take() {
            let db = f.state.db.clone();
            drop(f);
            tokio::task::spawn_blocking(move || db.shutdown())
                .await
                .unwrap();
        }
        // Mem contains only a private synchronous Store; no actor/socket/provider is spawned.
        self.mem.take();
    }
}

// Poll-boundary panic capture keeps cleanup reachable for setup/functional failures
// without spawning a second actor or adding a test dependency.
struct Catch<F: std::future::Future>(std::pin::Pin<Box<F>>);
impl<F: std::future::Future> std::future::Future for Catch<F> {
    type Output = std::result::Result<F::Output, Box<dyn std::any::Any + Send>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(std::task::Poll::Ready(v)) => std::task::Poll::Ready(Ok(v)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(e) => std::task::Poll::Ready(Err(e)),
        }
    }
}
async fn closed<T>(
    r: ContractRig,
    f: impl for<'a> FnOnce(
        &'a ContractRig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>,
) -> T {
    let outcome = Catch(Box::pin(f(&r))).await;
    let cleanup = Catch(Box::pin(r.close())).await;
    match (outcome, cleanup) {
        (Ok(v), Ok(())) => v,
        (Err(e), Ok(())) => std::panic::resume_unwind(e),
        (original, Err(_)) => panic!(
            "contract cleanup failed; original stage failed={}",
            original.is_err()
        ),
    }
}
#[tokio::test]
async fn s42_memory_and_database_admission_preserve_exact_route_and_whole_body_hold() {
    for memory in [true, false] {
        let (chain, holds, eligible, text, control) = closed(ContractRig::new(memory).await, |r| {
            Box::pin(async move {
                let (_, chain) = r.descendant().await;
                r.hold(&chain, &r.worker, &r.cap).await;
                r.post(
                    0,
                    collab::Sender::Human,
                    &r.cap,
                    "safe-looking suffix must not split held prefix",
                )
                .await;
                let control = r
                    .post(1, collab::Sender::Human, &r.cap, "independent channel")
                    .await;
                port_call!(r, prepare_causal_admission(&r.cap, crewd_wake::Ms(40))).unwrap();
                let rows = port_call!(r, pending(&r.cap)).unwrap();
                let rendered = port_call!(r, render_wake(&r.cap, &rows)).unwrap();
                (
                    chain,
                    port_call!(r, causal_holds(Some(&r.cap))).unwrap(),
                    rendered.cursor_targets,
                    rendered.text,
                    r.coordinate(&control),
                )
            })
        })
        .await;
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].chain_id, chain);
        assert_eq!(eligible, [(control.channel, control.seq)]);
        assert!(text.contains("independent channel"));
        assert!(!text.contains("actual descendant"));
        assert!(!text.contains("safe-looking suffix"));
    }
}
#[tokio::test]
async fn s42_memory_and_database_recovery_action_is_once_and_precise() {
    for memory in [true, false] {
        let (first, repeat, remaining, otherchain) = closed(ContractRig::new(memory).await, |r| {
            Box::pin(async move {
                let (_, chain) = r.descendant().await;
                r.hold(&chain, &r.worker, &r.cap).await;
                let other = r
                    .post(1, collab::Sender::Human, &r.worker, "other root")
                    .await;
                let otherchain = r.chain(&other).await;
                r.hold(&otherchain, &r.worker, &r.cap).await;
                let action = causal::HumanRecoveryAction::new();
                let first =
                    port_call!(r, recover_chain(&chain, &r.worker, &r.cap, &action)).unwrap();
                let repeat =
                    port_call!(r, recover_chain(&chain, &r.worker, &r.cap, &action)).unwrap();
                (
                    first,
                    repeat,
                    port_call!(r, causal_holds(Some(&r.cap))).unwrap(),
                    otherchain,
                )
            })
        })
        .await;
        assert!(first);
        assert!(!repeat);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].chain_id, otherchain);
    }
}
#[tokio::test]
async fn s42_memory_and_database_new_hold_after_prepare_blocks_claim_without_effects() {
    for memory in [true, false] {
        let (result, after, rows) = closed(ContractRig::new(memory).await, |r| {
            Box::pin(async move {
                let (_, chain) = r.descendant().await;
                let rows = port_call!(r, pending(&r.cap)).unwrap();
                let rendered = port_call!(r, render_wake(&r.cap, &rows)).unwrap();
                let b = Batch {
                    session: r.cap.clone(),
                    wakes: rows.clone(),
                    cursor_targets: rendered.cursor_targets,
                    prepared: rendered.prepared,
                    ..Default::default()
                };
                r.hold(&chain, &r.worker, &r.cap).await;
                (
                    port_call!(r, claim_reserved(&b, 7)).unwrap(),
                    port_call!(r, pending(&r.cap)).unwrap(),
                    rows,
                )
            })
        })
        .await;
        assert!(matches!(result, Decision::Busy));
        assert_eq!(after, rows, "final CAS Busy must not count claim attempts");
    }
}
#[tokio::test]
async fn s42_memory_sidecar_ack_fault_rolls_back_fate_cursor_and_cause() {
    let(fixture_error,fixture_record,sql_error,sql_unchanged,unchanged,before,held,ids,retry,finalrows)=closed(ContractRig::new(true).await,|r|Box::pin(async move{
        r.post(0,collab::Sender::Human,&r.worker,"ack original").await;let mut b=r.batch(&r.worker,3).await;
        let p=r.mem.as_ref().unwrap();let before=p.lock().cursors.clone();p.fail_next(MemFault::AckCursor);
        let cap=b.lease.as_ref().unwrap().causal_claimed().unwrap().clone();let fence=r.fence();fence.binder(&cap.pending,3).unwrap().register(61).unwrap();fence.observer(&r.worker,3).emitted(61);let(_,w)=b.lease.as_mut().unwrap().causal_witness_mut().unwrap();*w=Some(Box::new(fence.success(&cap.pending,3,61).unwrap()));
        let fixture_error=p.ack(&b,Ack::Delivered{riders:Riders::Release}).await.unwrap_err().to_string();
        let fixture_record={let st=p.lock();let hit=st.fault_hits.last().expect("consumed actual AckCursor");
            (hit.fault==MemFault::AckCursor) && hit.rows==hit.after_rows && hit.cursors==hit.after_cursors && hit.rows.iter().map(|w|w.id).collect::<Vec<_>>()==b.ids()};
        let unchanged=p.lock().cursors.clone();let held=p.lock().wakes.clone();
        let output_key=format!("causal.output.v1.{}",cap.pending.stable_key());
        let original={let model=p.causal.lock().unwrap();let value=crewd_store::meta_get(model.store.conn(),&output_key).unwrap();
            model.store.conn().execute_batch("CREATE TEMP TABLE s42_fail_key(key TEXT PRIMARY KEY); CREATE TEMP TRIGGER s42_ack_fail BEFORE INSERT ON meta WHEN NEW.key=(SELECT key FROM s42_fail_key) BEGIN SELECT RAISE(ABORT,'synthetic exact output commit failure'); END;").unwrap();
            model.store.conn().execute("INSERT INTO s42_fail_key(key) VALUES(?1)",[&output_key]).unwrap();value};
        let actual_error=p.ack(&b,Ack::Delivered{riders:Riders::Release}).await.unwrap_err();
        let sql_error=matches!(actual_error.downcast_ref::<collab::CollabError>(),Some(collab::CollabError::Store(crewd_store::StoreError::Sql(rusqlite::Error::SqliteFailure(code,Some(message)))))
            if code.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message=="synthetic exact output commit failure");
        let sql_unchanged={let model=p.causal.lock().unwrap();let unchanged=crewd_store::meta_get(model.store.conn(),&output_key).unwrap()==original;
            let rows=crewd_collab::wakes::by_ids(model.store.conn(),&b.ids()).unwrap();
            model.store.conn().execute_batch("DROP TRIGGER s42_ack_fail; DROP TABLE s42_fail_key").unwrap();
            unchanged && rows.iter().all(|w|w.state==crewd_collab::wakes::WakeState::Inflight)};
        let retry=p.ack(&b,Ack::Delivered{riders:Riders::Release}).await.unwrap();let finalrows=p.lock().wakes.clone();
        (fixture_error,fixture_record,sql_error,sql_unchanged,unchanged,before,held,b.ids(),retry,finalrows)
    })).await;
    assert_eq!(fixture_error, "synthetic one-shot AckCursor");
    assert!(fixture_record);
    assert!(
        sql_error,
        "must be exact SQLITE_CONSTRAINT_TRIGGER on original output receipt"
    );
    assert!(
        sql_unchanged,
        "actual SQL abort rolled back original output and rows"
    );
    assert_eq!(unchanged, before);
    assert!(
        held.iter()
            .any(|w| ids.contains(&w.id) && w.state == WakeState::Inflight)
    );
    assert!(!retry.stale);
    assert!(
        finalrows
            .iter()
            .filter(|w| ids.contains(&w.id))
            .all(|w| w.state == WakeState::Delivered)
    );
}
#[tokio::test]
async fn s42_memory_sidecar_boot_retains_hold_budget_and_rejects_old_cap() {
    let (stale, holds, chain, diagnostics) = closed(ContractRig::new(true).await, |r| {
        Box::pin(async move {
            let (_, chain) = r.descendant().await;
            let b = r.batch(&r.cap, 4).await;
            r.hold(&chain, &r.worker, &r.cap).await;
            let p = r.mem.as_ref().unwrap();
            p.fail_inflight("actual boot", crewd_wake::Ms(70))
                .await
                .unwrap();
            (
                p.settle_batch(&b, Settle::Requeue, crewd_wake::Ms(70))
                    .await
                    .unwrap(),
                p.causal_holds(Some(&r.cap)).await.unwrap(),
                chain,
                p.causal_diagnostics(&r.cap, crewd_wake::Ms(70))
                    .await
                    .unwrap(),
            )
        })
    })
    .await;
    assert!(matches!(stale, Decision::Stale));
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].chain_id, chain);
    assert!(diagnostics.iter().any(|(_, b)| !b.holds.is_empty()));
}

#[tokio::test]
async fn s42_memory_sparse_import_and_cross_channel_local_collisions_preserve_canonical_identity() {
    let (coordinates, globals, old_texts, facts, claim_targets) =
        closed(ContractRig::new(true).await, |r| {
            Box::pin(async move {
                let p = r.mem.as_ref().unwrap();
                {
                    let mut st = p.lock();
                    for (n, ch) in r.channels.iter().enumerate() {
                        for seq in [3, 10000] {
                            let text = format!("legacy {n} {seq}");
                            let facts = MsgFacts {
                                provenance: crewd_wake::plan::MessageProvenance::Ordinary,
                                id: format!("legacy:{n}:{seq}"),
                                at: MessageRef {
                                    channel: ch.clone(),
                                    seq,
                                },
                                sender: Sender::Human,
                                mentions: vec![],
                                created_at: Ms(0),
                            };
                            st.messages.insert((ch.clone(), seq), (facts, text));
                        }
                        st.cursors.insert((r.cap.clone(), ch.clone()), 10000);
                        st.cursors.insert((r.worker.clone(), ch.clone()), 10000);
                    }
                }
                let mut coordinates = Vec::new();
                let mut globals = Vec::new();
                let mut facts = Vec::new();
                for (n, ch) in r.channels.iter().enumerate() {
                    let (m, changes) = p
                        .post_causal(
                            collab::Sender::Human,
                            NewPost {
                                channel_id: ch.clone(),
                                text: format!("new {n}"),
                                mentions: vec![Mention {
                                    kind: MentionKind::Session,
                                    target_id: Some(r.cap.clone()),
                                }],
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    r.enqueue_posted(&m).await;
                    coordinates.push(p.causal_local_ref(&m).unwrap());
                    globals.push(m.seq);
                    facts.push(
                        changes
                            .iter()
                            .filter_map(|c| p.causal_fact(c).unwrap())
                            .filter(|f| matches!(f, CollabFact::MessageAppended { .. }))
                            .collect::<Vec<_>>(),
                    );
                }
                let old_texts = {
                    let st = p.lock();
                    r.channels
                        .iter()
                        .map(|ch| st.messages.get(&(ch.clone(), 10000)).unwrap().1.clone())
                        .collect::<Vec<_>>()
                };
                let batch = r.batch(&r.cap, 8).await;
                let claim_targets = batch.cursor_targets.clone();
                p.settle_batch(&batch, Settle::Requeue, Ms(0))
                    .await
                    .unwrap();
                (coordinates, globals, old_texts, facts, claim_targets)
            })
        })
        .await;
    assert_eq!(
        coordinates.iter().map(|at| at.seq).collect::<Vec<_>>(),
        [10001, 10001]
    );
    assert_ne!(
        globals[0], globals[1],
        "SQLite identities are global and distinct"
    );
    assert!(
        globals.iter().all(|g| *g < 10000),
        "small actual DB sequence must not overwrite sparse legacy local rows"
    );
    assert_eq!(old_texts, ["legacy 0 10000", "legacy 1 10000"]);
    for (at, events) in coordinates.iter().zip(facts) {
        assert_eq!(
            events,
            [CollabFact::MessageAppended {
                channel: at.channel.clone(),
                seq: at.seq
            }]
        );
    }
    assert_eq!(
        claim_targets,
        coordinates
            .iter()
            .map(|at| (at.channel.clone(), at.seq))
            .collect::<Vec<_>>()
    );
}

fn s42_diagnostic_contract_tables(
    c: &rusqlite::Connection,
) -> rusqlite::Result<Vec<(String, Vec<Vec<rusqlite::types::Value>>)>> {
    let mut names=c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') ORDER BY name")?;
    let names = names
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    names
        .into_iter()
        .map(|name| {
            let mut q = c.prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))?;
            let n = q.column_count();
            let mut rows = q
                .query_map([], |r| {
                    (0..n)
                        .map(|i| r.get(i))
                        .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.sort_by_key(|r| format!("{r:?}"));
            Ok((name, rows))
        })
        .collect()
}
#[tokio::test]
async fn s42_enabled_memory_readonly_missing_cursor_uses_shared_storage_diagnosis() {
    for memory in [true, false] {
        let (enabled, unknown, before, after, states, diagnostics, cursor, holds) =
            closed(ContractRig::new(memory).await, |r| {
                Box::pin(async move {
                    let m = r
                        .post(
                            0,
                            collab::Sender::Human,
                            &r.worker,
                            "S42_ENABLED_READONLY_MISSING_CURSOR",
                        )
                        .await;
                    let channel = m.channel_id.clone();
                    // Establishing actual substrate, then removing ONE cursor is explicit fixture
                    // corruption, never performed by diagnostics or replaced by a fake zero pin.
                    let (enabled, before, mem_before) = if let Some(p) = &r.mem {
                        let mut st = p.lock();
                        st.cursors.remove(&(r.worker.clone(), channel.clone()));
                        let mut model = p.causal.lock().unwrap();
                        model.sync(&st).unwrap();
                        (
                            model.enabled,
                            s42_diagnostic_contract_tables(model.store.conn()).unwrap(),
                            Some(format!(
                                "{:?}",
                                (
                                    &st.cursors,
                                    &st.wakes,
                                    &st.messages,
                                    &st.posted,
                                    &st.reservations
                                )
                            )),
                        )
                    } else {
                        let target = r.worker.clone();
                        let ch = channel.clone();
                        r.fixture
                            .as_ref()
                            .unwrap()
                            .state
                            .db
                            .call(move |s| {
                                s.conn().execute(
                                    "DELETE FROM read_cursors WHERE reader=?1 AND channel_id=?2",
                                    rusqlite::params![target, ch],
                                )
                            })
                            .await
                            .unwrap()
                            .unwrap();
                        let before = r
                            .fixture
                            .as_ref()
                            .unwrap()
                            .state
                            .db
                            .call(|s| s42_diagnostic_contract_tables(s.conn()))
                            .await
                            .unwrap()
                            .unwrap();
                        (true, before, None)
                    };
                    let unknown = if let Some(p) = &r.mem {
                        collab::causal::diagnostics(
                            p.causal.lock().unwrap().store.conn(),
                            &r.worker,
                            0,
                        )
                        .unwrap()
                        .unknown_channels
                    } else {
                        let target = r.worker.clone();
                        r.fixture
                            .as_ref()
                            .unwrap()
                            .state
                            .db
                            .call(move |s| collab::causal::diagnostics(s.conn(), &target, 0))
                            .await
                            .unwrap()
                            .unwrap()
                            .unknown_channels
                    };
                    let mut diagnostics = Vec::new();
                    for _ in 0..2 {
                        diagnostics
                            .push(port_call!(r, causal_diagnostics(&r.worker, Ms(0))).unwrap());
                    }
                    let holds = port_call!(r, causal_holds(Some(&r.worker))).unwrap();
                    let (after, mem_after, cursor) = if let Some(p) = &r.mem {
                        let st = p.lock();
                        let model = p.causal.lock().unwrap();
                        (
                            s42_diagnostic_contract_tables(model.store.conn()).unwrap(),
                            Some(format!(
                                "{:?}",
                                (
                                    &st.cursors,
                                    &st.wakes,
                                    &st.messages,
                                    &st.posted,
                                    &st.reservations
                                )
                            )),
                            collab::cursors::cursor(model.store.conn(), &r.worker, &channel)
                                .unwrap(),
                        )
                    } else {
                        let target = r.worker.clone();
                        let ch = channel.clone();
                        let (after, cursor) = r
                            .fixture
                            .as_ref()
                            .unwrap()
                            .state
                            .db
                            .call(move |s| {
                                Ok::<_, rusqlite::Error>((
                                    s42_diagnostic_contract_tables(s.conn())?,
                                    collab::cursors::cursor(s.conn(), &target, &ch).unwrap(),
                                ))
                            })
                            .await
                            .unwrap()
                            .unwrap();
                        (after, None, cursor)
                    };
                    (
                        enabled,
                        (channel, unknown),
                        before,
                        after,
                        (mem_before, mem_after),
                        diagnostics,
                        cursor,
                        holds,
                    )
                })
            })
            .await;
        assert!(
            enabled,
            "test must execute new enabled causal model, not legacy empty branch"
        );
        assert!(
            unknown.1.contains(&unknown.0),
            "actual shared storage reports eligible unknown cursor"
        );
        assert_eq!(cursor, None);
        assert_eq!(
            before, after,
            "repeated actual Port diagnostics cannot pin/hold/write receipt or durable state"
        );
        assert_eq!(
            states.0, states.1,
            "readonly enabled Memory model cannot mutate actual rows, cursors, messages or reservation"
        );
        assert!(holds.is_empty());
        for d in diagnostics {
            let (_, b) = d
                .iter()
                .find(|(ch, _)| ch == &unknown.0)
                .expect("S42 enabled Mem/Db unknown channel must survive actual shared adapter");
            assert!(b.cursor_unknown);
            assert!(
                b.holds.is_empty() && b.hold_candidates.is_empty() && b.unresolved.is_empty(),
                "unknown cursor must not fabricate a root/message/budget hold"
            );
        }
    }
}
