//! Public frozen-Todo regression; synthetic data, real attached socket, no real runner.
use crewd_collab::{
    Locale,
    refusal::{Code, Refusal},
};
use crewd_proto::{MentionKind, Session, SessionLive};
use crewd_server::agent::port;
use crewd_server::control::*;
use crewd_server::{
    AppState,
    agent::{
        testkit::{Fixture, RpcClient},
        text,
    },
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn frozen_todo_write_is_refused_on_an_already_attached_connection() {
    let f = Fixture::new().await;
    f.add_agent_todo(1, "synthetic frozen todo").await;
    let mut rpc = RpcClient::connect(&f.spawn_socket().await).await;
    rpc.call(
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await
    .unwrap();
    let active = rpc
        .call(
            "agent.call",
            json!({"tool":"respond_todo","args":{"number":1,"response":"active response"}}),
        )
        .await
        .unwrap();
    assert_eq!(active["isError"], false);
    f.rpc("crew.remove", json!({"crewId":f.crew})).await;
    let before = f
        .state
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT updated_at FROM todos WHERE number=1", [], |r| {
                    r.get::<_, Option<String>>(0)
                })
        })
        .await
        .unwrap()
        .unwrap();
    let v = rpc
        .call(
            "agent.call",
            json!({"tool":"respond_todo","args":{"number":1,"response":"after freeze"}}),
        )
        .await
        .unwrap();
    let r: crewd_proto::agent::CallResult = serde_json::from_value(v).unwrap();
    assert!(r.is_error, "frozen Todo write was accepted: {}", r.text);
    let refusal = Refusal::new(Code::CrewRemoved)
        .arg("crew", f.crew.as_str())
        .text(Locale::Zh);
    assert_eq!(
        r.text,
        text::r(Locale::Zh, "r.failed", &[("detail", &refusal)])
    );
    let after = f
        .state
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT updated_at FROM todos WHERE number=1", [], |r| {
                    r.get::<_, Option<String>>(0)
                })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    let count: i64 = f
        .state
        .db
        .call(|s| {
            s.conn().query_row(
                "SELECT COUNT(*) FROM todo_responses WHERE text='after freeze'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 0);
    let read = rpc
        .call(
            "agent.call",
            json!({"tool":"read_channel","args":{"channel":format!("{}/lobby",f.crew)}}),
        )
        .await
        .unwrap();
    assert_eq!(
        read["isError"], false,
        "freeze must not remove valid read access"
    );
}

async fn run(st: &AppState, who: &str, name: &str, args: Value) -> crewd_proto::agent::CallResult {
    let me = crewd_server::agent::port::identity(st, who)
        .await
        .unwrap()
        .unwrap();
    crewd_server::agent::tools::call(st, &me, name, args).await
}

#[tokio::test]
async fn implicit_and_explicit_human_mentions_share_one_reservation() {
    for mode in ["implicit", "explicit", "mixed"] {
        let f = Fixture::new().await;
        let lobby = crewd_server::agent::port::lobby_of(&f.state, &f.crew)
            .await
            .unwrap();
        let h = f
            .human_post(&lobby.id, "synthetic human question", vec![])
            .await;
        for i in 0..4 {
            let mut a = json!({"message":format!("{mode}-answer-{i}")});
            if mode != "explicit" {
                a["reply_to"] = json!(h.seq);
            } else {
                a["channel"] = json!(lobby.id);
            }
            if mode == "explicit" || (mode == "mixed" && i % 2 == 1) {
                a["mentions"] = json!([{"kind":"human"}]);
            }
            let r = run(&f.state, &f.worker, "post", a).await;
            assert_eq!(r.is_error, i == 3, "{mode} {i}: {}", r.text);
            if i == 3 {
                assert_eq!(
                    r.text,
                    crewd_server::agent::text::r(Locale::Zh, "r.human_mention_throttled", &[])
                );
            }
        }
        let rows = crewd_server::agent::port::messages_raw(&f.state, &lobby.id, None, 20)
            .await
            .unwrap();
        let sent: Vec<_> = rows
            .iter()
            .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
            .collect();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|m| {
            m.mentions
                .iter()
                .filter(|a| a.kind == MentionKind::Human)
                .count()
                == 1
        }));
    }
}

#[tokio::test]
async fn implicit_human_reservation_refunds_failed_first_write_but_not_partial_success() {
    for partial in [false, true] {
        let f = Fixture::new().await;
        let lobby = crewd_server::agent::port::lobby_of(&f.state, &f.crew)
            .await
            .unwrap();
        let h = f.human_post(&lobby.id, "synthetic human", vec![]).await;
        let huge = "x".repeat(300 * 1024);
        let args = if partial {
            json!({"reply_to":h.seq,"messages":[{"text":"partial first"},{"text":huge},{"text":"never sent"}]})
        } else {
            json!({"reply_to":h.seq,"message":huge})
        };
        assert!(run(&f.state, &f.worker, "post", args).await.is_error);
        for i in 0..(3 - usize::from(partial)) {
            let args = json!({"reply_to":h.seq,"message":format!("valid-{i}")});
            let r = run(&f.state, &f.worker, "post", args.clone()).await;
            assert!(!r.is_error, "{partial}/{i}: {}", r.text);
            if i == 0 {
                let duplicate = run(&f.state, &f.worker, "post", args).await;
                assert!(duplicate.is_error, "duplicate unexpectedly persisted");
            }
        }
        let r = run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"fourth human ping"}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.human_mention_throttled", &[])
        );
        let rows = crewd_server::agent::port::messages_raw(&f.state, &lobby.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
                .count(),
            3
        );
    }
}

#[tokio::test]
async fn concurrent_implicit_human_mentions_are_serialized_before_reservation() {
    let f = Fixture::new().await;
    let lobby = crewd_server::agent::port::lobby_of(&f.state, &f.crew)
        .await
        .unwrap();
    let h = f
        .human_post(&lobby.id, "synthetic concurrent question", vec![])
        .await;
    let (a, b, c, d) = tokio::join!(
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-a"})
        ),
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-b"})
        ),
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-c"})
        ),
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-d"})
        ),
    );
    assert_eq!([a, b, c, d].iter().filter(|r| r.is_error).count(), 1);
    let rows = crewd_server::agent::port::messages_raw(&f.state, &lobby.id, None, 20)
        .await
        .unwrap();
    let replies: Vec<_> = rows
        .iter()
        .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
        .collect();
    assert_eq!(replies.len(), 3);
    assert!(replies.iter().all(|m| {
        m.mentions
            .iter()
            .filter(|x| x.kind == MentionKind::Human)
            .count()
            == 1
    }));
}

// Persists the real collab session row; only process/control outcomes are synthetic.
struct PersistedControl {
    backing: Arc<AppState>,
    fake: FakeSessionControl,
    starts: Mutex<Vec<StartRequest>>,
}
impl SessionControl for PersistedControl {
    fn start(&self, r: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>> {
        self.starts.lock().unwrap().push(r.clone());
        Box::pin(async move {
            self.backing
                .collab(move |tx| {
                    crewd_collab::sessions::register(
                        tx,
                        crewd_collab::sessions::NewSession {
                            crew_id: r.crew_id,
                            role: r.role,
                            executor: r.executor,
                            title: r.title,
                            brief: Some(r.brief),
                            ..Default::default()
                        },
                    )
                })
                .await
                .map_err(|e| ControlError::Failed(e.to_string()))
        })
    }
    fn resume(&self, s: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.fake.resume(s)
    }
    fn stop(&self, s: String, r: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.fake.stop(s, r)
    }
    fn nudge(&self, s: String, t: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.fake.nudge(s, t)
    }
    fn inspect(&self, s: String, n: usize) -> BoxFuture<'_, Result<Inspection, ControlError>> {
        self.fake.inspect(s, n)
    }
    fn set_profile(
        &self,
        s: String,
        m: Option<String>,
        e: Option<String>,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        self.fake.set_profile(s, m, e)
    }
    fn handoff_captain(&self, c: String, s: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.fake.handoff_captain(c, s)
    }
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>> {
        self.fake.quota()
    }
    fn live(&self, s: String) -> BoxFuture<'_, Result<SessionLive, ControlError>> {
        self.fake.live(s)
    }
}
fn persisted(f: &Fixture) -> (AppState, Arc<PersistedControl>) {
    let c = Arc::new(PersistedControl {
        backing: f.state.clone(),
        fake: Default::default(),
        starts: Default::default(),
    });
    (
        AppState::new(f.state.db.clone()).with_sessions(c.clone()),
        c,
    )
}

async fn locale(f: &Fixture, l: Locale) {
    f.state
        .collab(move |tx| crewd_collab::settings::set_locale(tx, l))
        .await
        .unwrap();
}
// Parse the actual readable call's quoted JSON values (not an independently invented retry).
fn advertised(text: &str, tool: &str) -> Value {
    let tail = text
        .split_once(&format!("{tool}("))
        .expect("receipt must advertise a callable route")
        .1;
    let body = tail.split_once(')').unwrap().0;
    let mut rest = body;
    let mut args = serde_json::Map::new();
    while !rest.trim().is_empty() {
        let (key, value) = rest.trim_start().split_once(':').unwrap();
        let mut parsed =
            serde_json::Deserializer::from_str(value.trim_start()).into_iter::<Value>();
        let v = parsed.next().unwrap().unwrap();
        let used = parsed.byte_offset();
        args.insert(key.trim().into(), v);
        rest = value.trim_start()[used..]
            .trim_start()
            .trim_start_matches(',');
    }
    Value::Object(args)
}

#[tokio::test]
async fn child_handoff_failure_advertises_a_usable_retry_without_widening_stop_authority() {
    for l in [Locale::Zh, Locale::En] {
        let f = Fixture::new().await;
        locale(&f, l).await;
        let child = f.add_crew("child", Some(&f.crew)).await;
        let (st, ctl) = persisted(&f);
        ctl.fake.fail_method(
            FakeMethod::Handoff,
            ControlError::Failed("synthetic handoff failure".into()),
        );
        let r = run(
            &st,
            &f.captain,
            "handoff_captain",
            json!({"crew":child,"runner":"claude","title":"new captain"}),
        )
        .await;
        assert!(r.is_error);
        let rows = port::sessions_of(&st, &child).await.unwrap();
        assert_eq!(rows.len(), 1);
        let sid = &rows[0].id;
        let args = advertised(&r.text, "handoff_captain");
        let session_arg = serde_json::to_string(sid).unwrap();
        assert!(
            r.text.contains(&text::r(
                l,
                "r.handoff_cleanup_authority",
                &[("session_arg", &session_arg)]
            )),
            "cleanup must be directed to an authorized actor: {}",
            r.text
        );
        assert!(
            !r.text.contains("stop_session("),
            "ordinary parent must not be told to execute child stop"
        );
        assert!(
            args.get("crew").is_some(),
            "retry lost resolved child crew: {}",
            r.text
        );
        assert_eq!(args["to_session"], *sid);
        let retry = run(&st, &f.captain, "handoff_captain", args).await;
        assert!(!retry.is_error, "{}", retry.text);
        assert_eq!(ctl.starts.lock().unwrap().len(), 1);
        assert_eq!(
            ctl.fake
                .calls()
                .iter()
                .filter(|c| matches!(c, FakeCall::Handoff(..)))
                .count(),
            2
        );
        let stop = run(
            &st,
            &f.captain,
            "stop_session",
            json!({"session_id":sid,"reason":"not wanted"}),
        )
        .await;
        assert!(stop.is_error);
        assert!(
            !ctl.fake
                .calls()
                .iter()
                .any(|c| matches!(c, FakeCall::Stop(..)))
        );
    }
}

#[tokio::test]
async fn own_and_management_handoff_recovery_can_execute_retry_and_cleanup() {
    for l in [Locale::Zh, Locale::En] {
        for management in [false, true] {
            for unnumbered in [false, true] {
                let f = Fixture::new().await;
                locale(&f, l).await;
                let target = if management {
                    f.add_crew("child", Some(&f.crew)).await
                } else {
                    f.crew.clone()
                };
                let who = if management {
                    let mgmt = f.management_crew().await;
                    f.add_session(
                        &mgmt,
                        "s-management",
                        crewd_server::agent::Role::Captain,
                        "synthetic boss",
                    )
                    .await;
                    "s-management".to_string()
                } else {
                    f.captain.clone()
                };
                if unnumbered {
                    let id = target.clone();
                    f.state
                        .db
                        .call(move |s| {
                            s.conn()
                                .execute("UPDATE crews SET number=NULL WHERE id=?1", [id])
                        })
                        .await
                        .unwrap()
                        .unwrap();
                }
                let (st, ctl) = persisted(&f);
                ctl.fake.fail_method(
                    FakeMethod::Handoff,
                    ControlError::Failed("synthetic failure".into()),
                );
                let r = run(
                    &st,
                    &who,
                    "handoff_captain",
                    json!({"crew":target,"runner":"claude","title":"new captain"}),
                )
                .await;
                assert!(r.is_error);
                let args = advertised(&r.text, "handoff_captain");
                if unnumbered {
                    assert_eq!(args["crew"], target);
                }
                assert!(!run(&st, &who, "handoff_captain", args).await.is_error);
                assert_eq!(ctl.starts.lock().unwrap().len(), 1);
                let cleanup = run(
                    &st,
                    &who,
                    "stop_session",
                    advertised(&r.text, "stop_session"),
                )
                .await;
                assert!(!cleanup.is_error, "{}", cleanup.text);
                assert_eq!(
                    ctl.fake
                        .calls()
                        .iter()
                        .filter(|c| matches!(c, FakeCall::Stop(..)))
                        .count(),
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn already_closed_authority_and_failed_parent_recovery_remain_closed() {
    let f = Fixture::new().await;
    let unrelated = f.add_crew("unrelated", None).await;
    assert!(
        run(
            &f.state,
            &f.captain,
            "crew_structure",
            json!({"action":"adopt","target":unrelated})
        )
        .await
        .is_error
    );
    assert!(
        port::crew(&f.state, &unrelated)
            .await
            .unwrap()
            .unwrap()
            .parent_ids
            .is_empty()
    );
    assert!(
        run(
            &f.state,
            &f.captain,
            "handoff_captain",
            json!({"crew":unrelated,"runner":"claude"})
        )
        .await
        .is_error
    );
    assert!(f.control_calls().is_empty());
    f.fail_method(
        FakeMethod::Start,
        ControlError::Failed("synthetic start failure".into()),
    );
    let r = run(
        &f.state,
        &f.captain,
        "crew_structure",
        json!({"action":"create_parent","title":"parent"}),
    )
    .await;
    assert!(r.is_error);
    assert!(r.text.contains("post(to: \"0\")"));
    let parent = port::crew(&f.state, &f.crew)
        .await
        .unwrap()
        .unwrap()
        .parent_ids[0]
        .clone();
    assert!(
        port::crew(&f.state, &parent)
            .await
            .unwrap()
            .unwrap()
            .captain
            .is_none()
    );
    assert!(
        !port::acts_as_captain_of(&f.state, &f.captain, &parent)
            .await
            .unwrap()
    );
}
