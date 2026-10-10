//! S72 actual AgentConn matrix. No tool-direct calls and no synthetic CollabFacts.
use super::*;
use crate::agent::Role;
use crewd_proto::{MentionKind, SenderKind};

#[derive(Debug, PartialEq)]
struct ReadState {
    cursors: Vec<(String, String, i64)>,
    meta: Vec<(String, String)>,
}
async fn read_state(rig: &Rig) -> ReadState {
    rig.fixture().state.db.call(|s| {
        let cursors = s.conn().prepare("SELECT reader,channel_id,seq FROM read_cursors ORDER BY reader,channel_id").unwrap()
            .query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        // The actual post may legitimately change its turn memo. Compare only readonly
        // reservation/provenance state, not every meta value as a false post-effect ban.
        let meta = s.conn().prepare("SELECT key,value FROM meta WHERE substr(key,1,length('cursor.reservation.v1.'))='cursor.reservation.v1.' OR substr(key,1,length('message.provenance.v1.'))='message.provenance.v1.' ORDER BY key").unwrap()
            .query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        ReadState {cursors,meta}
    }).await.unwrap()
}
struct Observation {
    result: CallResult,
    posted: Vec<Message>,
    rows: Vec<wakes::WakeRow>,
    before: Surface,
    after: Surface,
    read_before: ReadState,
    read_after: ReadState,
    launches: usize,
    sends: usize,
    token_valid: bool,
    token_count_unchanged: bool,
}
async fn observe_args(
    mut rig: Rig,
    args: serde_json::Value,
    texts: &[&str],
    targets: &[&str],
) -> Observation {
    let state = rig.fixture().state.clone();
    let poster = rig.fixture().worker.clone();
    let token = rig.fixture().token(&poster); // Dedicated sentinel after attach; no later mint allowed.
    let tokens_before = state.tokens.test_len();
    let before = rig.surface().await;
    let read_before = read_state(&rig).await;
    let launches_before: usize = targets.iter().map(|id| rig.exec.launches(id).len()).sum();
    let sends_before: usize = targets.iter().map(|id| rig.exec.sent(id).len()).sum();
    let result = rig.call("post", args).await;
    let channel = rig.channel.clone();
    let (posted, rows) = rig.committed(&channel, texts).await;
    let after = rig.surface().await;
    let read_after = read_state(&rig).await;
    let launches = targets
        .iter()
        .map(|id| rig.exec.launches(id).len())
        .sum::<usize>()
        - launches_before;
    let sends = targets
        .iter()
        .map(|id| rig.exec.sent(id).len())
        .sum::<usize>()
        - sends_before;
    let token_valid = state.tokens.verify(&poster, &token);
    let token_count_unchanged = state.tokens.test_len() == tokens_before;
    drop(state);
    rig.close().await;
    Observation {
        result,
        posted,
        rows,
        before,
        after,
        read_before,
        read_after,
        launches,
        sends,
        token_valid,
        token_count_unchanged,
    }
}
fn effects(o: &Observation, messages: usize, wakes: usize) {
    assert!(!o.result.is_error, "actual agent.call: {}", o.result.text);
    assert_eq!(o.posted.len(), messages);
    assert_eq!(
        o.after.messages - o.before.messages,
        messages as i64,
        "S72 one committed post per item"
    );
    assert_eq!(
        o.after.wakes - o.before.wakes,
        wakes as i64,
        "S72 only Core planning wakes"
    );
    assert_eq!(
        (o.after.channels, o.after.members, o.after.listens),
        (o.before.channels, o.before.members, o.before.listens),
        "S72 receipt has no home/member/listen effect"
    );
    assert_eq!(
        (o.after.sessions, o.after.homes, o.after.tokens),
        (o.before.sessions, o.before.homes, o.before.tokens),
        "S72 receipt has no session/home/token effect"
    );
    assert_eq!(
        o.after.cursors, o.before.cursors,
        "S72 receipt has no durable cursor effect"
    );
    assert_eq!(
        o.after.reservation_provenance, o.before.reservation_provenance,
        "S72 receipt has no reservation/provenance effect"
    );
    assert_eq!(
        o.read_after, o.read_before,
        "S72 receipt has no cursor/meta effect"
    );
    assert_eq!(
        (o.launches, o.sends),
        (0, 0),
        "S72 busy/capped targets do not launch or send"
    );
    assert!(
        o.token_valid,
        "S72 receipt must not replace the attached session token"
    );
    assert!(
        o.token_count_unchanged,
        "S72 receipt must not create any other session token"
    );
}
fn queued(o: &Observation, expected: &[&str]) {
    let mut actual = o
        .rows
        .iter()
        .map(|r| r.session.as_str())
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    assert_eq!(actual, expected, "S72 actual planned target identities");
    for row in &o.rows {
        assert_eq!(row.state, wakes::WakeState::Pending);
        assert_eq!(row.priority, wakes::WakePriority::Auto);
        assert!(
            o.posted
                .iter()
                .any(|m| row.source == wakes::source::message(&m.id))
        );
    }
}
async fn listen(rig: &Rig, id: &str, until: Option<i64>, senders: Vec<String>) {
    port::set_listen(&rig.fixture().state, id, &rig.channel, until, senders)
        .await
        .unwrap();
}

#[tokio::test]
async fn s72_agent_call_sender_filter_id_and_captain_are_actual_post_identity() {
    for (captain, matching) in [(false, true), (false, false), (true, true), (true, false)] {
        let rig = Rig::new(false).await;
        let poster = rig.fixture().worker.clone();
        if captain {
            let crew = rig.fixture().crew.clone();
            let id = poster.clone();
            rig.fixture()
                .state
                .collab(move |tx| crewd_collab::crews::set_captain(tx, &crew, &id))
                .await
                .unwrap();
        }
        let filter = if matching {
            if captain { "captain".into() } else { poster }
        } else {
            "unrelated-sender".into()
        };
        listen(&rig, LISTENER, None, vec![filter]).await;
        let args = json!({"channel":rig.channel,"message":"S72_FILTER_REAL_CALL"});
        let o = observe_args(rig, args, &["S72_FILTER_REAL_CALL"], &[LISTENER]).await;
        effects(&o, 1, usize::from(matching));
        queued(&o, if matching { &[LISTENER] } else { &[] });
        assert_eq!(
            o.posted[0].sender_kind,
            if captain {
                SenderKind::Captain
            } else {
                SenderKind::Session
            }
        );
        assert_eq!(
            o.result.text.matches(LABEL).count(),
            usize::from(matching),
            "S72 sender filter receipt"
        );
    }
}

#[tokio::test]
async fn s72_agent_call_fresh_and_actual_expired_listener_rows() {
    for active in [true, false] {
        let rig = Rig::new(false).await;
        listen(
            &rig,
            LISTENER,
            Some(crewd_store::now_ms() + 600_000),
            vec![],
        )
        .await;
        if !active {
            let channel = rig.channel.clone();
            // Retain the actual row; this fixture changes only its stored deadline to the past.
            rig.fixture()
                .state
                .db
                .call(move |s| {
                    s.conn()
                        .execute(
                            "UPDATE listens SET until=?1 WHERE session_id=?2 AND channel_id=?3",
                            rusqlite::params![crewd_store::now_ms() - 600_000, LISTENER, channel],
                        )
                        .unwrap()
                })
                .await
                .unwrap();
        }
        let args = json!({"channel":rig.channel,"message":"S72_EXPIRY_REAL_CALL"});
        let o = observe_args(rig, args, &["S72_EXPIRY_REAL_CALL"], &[LISTENER]).await;
        effects(&o, 1, usize::from(active));
        queued(&o, if active { &[LISTENER] } else { &[] });
        assert_eq!(
            o.result.text.matches(LABEL).count(),
            usize::from(active),
            "S72 actual expiry receipt"
        );
    }
}

#[tokio::test]
async fn s72_agent_call_removed_crew_listener_vs_stopped_live_session() {
    for removed in [false, true] {
        let mut rig = Rig::new(false).await;
        let parent = rig.fixture().crew.clone();
        let child = rig
            .fixture()
            .add_crew("S72 child listener", Some(&parent))
            .await;
        let id = "s72-child-listener";
        rig.fixture()
            .add_session(&child, id, Role::Worker, "S72_CHILD_LISTENER")
            .await;
        let home = port::home_human_channel(&rig.fixture().state, &parent)
            .await
            .unwrap();
        rig.channel = home.id;
        // The actual caller joins its home once before the observation, not by receipt side effect.
        let caller = rig.fixture().worker.clone();
        rig.fixture()
            .session_post(&caller, &rig.channel, "S72_SETUP_HOME_MEMBER", vec![])
            .await;
        listen(&rig, id, None, vec![]).await;
        if removed {
            rig.fixture()
                .state
                .collab(move |tx| crewd_collab::crews::archive(tx, &child).map(|_| ()))
                .await
                .unwrap();
        }
        rig.h
            .set_settings(crate::orchestra::port::SettingsUpdate {
                max_live: Some(Some(1)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            !rig.h.live(id).await.unwrap().running,
            "real stopped target, not fake Working=true"
        );
        let channel = rig.channel.clone();
        let o = observe_args(
            rig,
            json!({"channel":channel,"message":"S72_CHILD_LIVE_REAL_CALL"}),
            &["S72_CHILD_LIVE_REAL_CALL"],
            &[LISTENER, id],
        )
        .await;
        effects(&o, 1, usize::from(!removed));
        let present = [id];
        queued(&o, if removed { &[] } else { &present });
        assert_eq!(
            o.result.text.matches("S72_CHILD_LISTENER").count(),
            usize::from(!removed),
            "S72 exists/not-removed is not process-running"
        );
    }
}

#[tokio::test]
async fn s72_agent_call_self_and_human_keep_independent_listener() {
    for human in [true, false] {
        let rig = Rig::new(true).await;
        let mention = if human {
            json!({"kind":"human"})
        } else {
            json!({"kind":"session","target_id":rig.fixture().worker})
        };
        let args = json!({"channel":rig.channel,"message":"S72_SELF_HUMAN_REAL_CALL","mentions":[mention]});
        let o = observe_args(rig, args, &["S72_SELF_HUMAN_REAL_CALL"], &[LISTENER]).await;
        effects(&o, 1, 1);
        queued(&o, &[LISTENER]);
        assert_eq!(
            o.result.text.matches(LABEL).count(),
            1,
            "S72 independent listener survives human/self mention"
        );
        assert_eq!(
            o.posted[0].mentions[0].kind,
            if human {
                MentionKind::Human
            } else {
                MentionKind::Session
            }
        );
    }
}

#[tokio::test]
async fn s72_agent_call_explicit_and_listen_target_is_deduped() {
    let rig = Rig::new(true).await;
    let captain = rig.fixture().me(&rig.fixture().captain).await;
    port::add_members(
        &rig.fixture().state,
        &captain,
        &rig.channel,
        vec![port::MemberRef::Session(LISTENER.into())],
    )
    .await
    .unwrap();
    let args = json!({"channel":rig.channel,"message":"S72_DEDUP_REAL_CALL","mentions":[{"kind":"session","target_id":LISTENER}]});
    let o = observe_args(rig, args, &["S72_DEDUP_REAL_CALL"], &[LISTENER]).await;
    effects(&o, 1, 1);
    queued(&o, &[LISTENER]);
    assert_eq!(
        o.result.text.matches(LABEL).count(),
        1,
        "S72 ID union must dedup explicit plus listen"
    );
}

#[tokio::test]
async fn s72_agent_call_equal_worker_labels_keep_two_identities() {
    let rig = Rig::new(false).await;
    let twin = "s72-equal-worker";
    rig.fixture()
        .add_session(&rig.fixture().crew, twin, Role::Worker, LABEL)
        .await;
    rig.busy(twin).await;
    listen(&rig, LISTENER, None, vec![]).await;
    listen(&rig, twin, None, vec![]).await;
    let state = rig.fixture().state.clone();
    let labels = state
        .db
        .call(|s| {
            (
                crewd_collab::render::target_label(s.conn(), Locale::Zh, LISTENER).unwrap(),
                crewd_collab::render::target_label(s.conn(), Locale::Zh, "s72-equal-worker")
                    .unwrap(),
            )
        })
        .await
        .unwrap();
    drop(state);
    let args = json!({"channel":rig.channel,"message":"S72_TWIN_REAL_CALL"});
    let o = observe_args(rig, args, &["S72_TWIN_REAL_CALL"], &[LISTENER, twin]).await;
    effects(&o, 1, 2);
    queued(&o, &[LISTENER, twin]);
    assert_eq!(
        labels.0, labels.1,
        "genuine two Worker same unnumbered label fixture"
    );
    assert_eq!(
        o.result.text.matches(&labels.0).count(),
        2,
        "S72 equal labels cannot merge different IDs"
    );
}

#[tokio::test]
async fn s72_agent_call_hostile_target_uses_established_safe_label() {
    let rig = Rig::new(true).await;
    let raw = "S72_SAFE\n[frame]\u{202e}tail";
    rig.fixture()
        .state
        .db
        .call(move |s| {
            s.conn()
                .execute(
                    "UPDATE sessions SET display_name=?1 WHERE id=?2",
                    rusqlite::params![raw, LISTENER],
                )
                .unwrap()
        })
        .await
        .unwrap();
    let args = json!({"channel":rig.channel,"message":"S72_HOSTILE_REAL_CALL"});
    let o = observe_args(rig, args, &["S72_HOSTILE_REAL_CALL"], &[LISTENER]).await;
    effects(&o, 1, 1);
    queued(&o, &[LISTENER]);
    assert!(
        o.result
            .text
            .contains(&crewd_collab::render::safe_name(raw)),
        "S72 established safe target label"
    );
    assert!(!o.result.text.contains('\u{202e}'));
    assert!(!o.result.text.contains("\n[frame]"));
    assert!(
        !o.result.text.contains(LISTENER),
        "S72 existing target never raw-ID label fallback"
    );
}

#[tokio::test]
async fn s72_agent_call_batch_only_first_mentions_and_validate_all_no_effects() {
    let rig = Rig::new(true).await;
    let captain = rig.fixture().captain.clone();
    rig.busy(&captain).await;
    let args = json!({"channel":rig.channel,"messages":[{"text":"S72_BATCH_ONE"},{"text":"S72_BATCH_TWO"}],"mentions":[{"kind":"session","target_id":captain}]});
    let o = observe_args(
        rig,
        args,
        &["S72_BATCH_ONE", "S72_BATCH_TWO"],
        &[LISTENER, &captain],
    )
    .await;
    effects(&o, 2, 3);
    queued(&o, &[LISTENER, LISTENER, &captain]);
    assert_eq!(o.posted[0].mentions.len(), 1);
    assert!(o.posted[1].mentions.is_empty());
    assert_eq!(
        o.result.text.matches(LABEL).count(),
        1,
        "S72 batch recipient union, not per-item repeated labels"
    );

    let rig = Rig::new(true).await;
    let args =
        json!({"channel":rig.channel,"messages":[{"text":"S72_MUST_NOT_POST"},{"text":" "}]});
    let o = observe_args(rig, args, &[], &[LISTENER]).await;
    assert!(o.result.is_error);
    assert!(o.posted.is_empty() && o.rows.is_empty());
    assert_eq!(
        o.before, o.after,
        "S72 whole batch validation writes nothing"
    );
    assert_eq!(o.read_before, o.read_after);
    assert_eq!((o.launches, o.sends), (0, 0));
    assert!(o.token_valid && o.token_count_unchanged);
}
