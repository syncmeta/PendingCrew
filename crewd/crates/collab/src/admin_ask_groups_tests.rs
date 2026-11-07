use super::*;
use crate::testkit::{NOW, count, ok, ok_at, store};
use crewd_proto::{
    Ask, AskDefault,
    judgments::{JudgmentConfig, JudgmentSettingsSet},
};
use crewd_store::Store;

fn fixture() -> Store {
    let mut s = store();
    // Domain-only setup calls the real next migration inside a transaction.
    // It intentionally leaves the current version untouched: this is NOT cap qualification.
    if !db::available(s.conn()).unwrap() {
        ok(&mut s, |tx| {
            db::migrate(tx.conn())?;
            Ok(())
        });
    }
    configure(&mut s, true);
    s
}
fn configure(s: &mut Store, enabled: bool) {
    let expected_revision = judgments::configuration(s.conn()).unwrap().revision;
    ok(s, |tx| {
        judgments::set_configuration(
            tx,
            &Sender::Human,
            JudgmentSettingsSet {
                request_id: uuid::Uuid::new_v4().to_string(),
                expected_revision,
                credential_update: Default::default(),
                config: Some(JudgmentConfig {
                    enabled: true,
                    provider: "typesafe".into(),
                    base_url: "https://example.invalid".into(),
                    model: "synthetic".into(),
                    choice_min: 0.5,
                    wake_yes: 0.5,
                    similar_requirements: None,
                    similar_asks: enabled.then_some(true),
                    credential_source: "file".into(),
                }),
            },
        )?;
        crewd_store::meta_set(tx.conn(), judgments::ACTIVE_KEY, "synthetic-domain-fixture")?;
        Ok(())
    });
}
fn question(s: &mut Store, session: &str, text: &str, default: bool) -> Ask {
    ok(s, |tx| {
        let opened = asks::open(
            tx,
            session,
            asks::OpenAsk {
                question: text.into(),
                default: default.then(|| AskDefault {
                    text: "wait for explicit confirmation".into(),
                    after_minutes: 1,
                }),
                resume_note: Some("original work context remains private".into()),
            },
        )?;
        if !db::available(tx.conn())? {
            note_source_changes(
                tx,
                &[crate::Change::AskChanged {
                    crew_id: opened.crew_id.clone(),
                    ask: opened.ask.clone(),
                }],
            )?;
        }
        Ok(opened)
    })
}
fn prepared(s: &mut Store) -> Snapshot {
    for _ in 0..8 {
        ok(s, observe_page);
    }
    ok(s, next_dispatch).expect("a real pair of current original questions")
}
fn listing(s: &Store, crew: &str) -> Page {
    list(
        s.conn(),
        &Sender::Human,
        List {
            crew_id: crew.into(),
            after: None,
            limit: Some(20),
        },
        NOW,
    )
    .unwrap()
}
fn business(s: &Store) -> (String, i64, i64, i64) {
    (
        serde_json::to_string(asks::load_org(s.conn()).unwrap().state()).unwrap(),
        count(s, "SELECT count(*) FROM messages"),
        count(s, "SELECT count(*) FROM wakes"),
        count(s, "SELECT count(*) FROM organization_work_events"),
    )
}

#[test]
fn actual_home_announcements_group_without_mutating_any_original_or_wait() {
    let mut s = fixture();
    let a = question(
        &mut s,
        "w-a1",
        "Should we publish the validated report?",
        true,
    );
    let b = question(
        &mut s,
        "w-a2",
        "May the validated report be published?",
        false,
    );
    let before = business(&s);
    let shot = prepared(&mut s);
    assert_eq!(
        (shot.first.id.as_str(), shot.second.id.as_str()),
        (a.ask.as_str(), b.ask.as_str())
    );
    assert_eq!(
        channels::get(s.conn(), &shot.first.source_channel_id)
            .unwrap()
            .kind,
        ChannelKind::Human
    );
    assert_eq!(
        crate::messages::get(s.conn(), &shot.first.source_message_id)
            .unwrap()
            .text,
        a.question
    );
    ok(&mut s, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 0.9,
                suggested: true,
            },
        )
    });
    let page = listing(&s, "crew-a");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].state, State::Suggested);
    assert_eq!(before, business(&s));
    assert_eq!(asks::get(s.conn(), "crew-a", &a.ask).unwrap(), a);
    assert_eq!(asks::get(s.conn(), "crew-a", &b.ask).unwrap(), b);
}

#[test]
fn answer_and_default_deadline_invalidate_held_results_without_touching_neighbor() {
    let mut s = fixture();
    let a = question(&mut s, "w-a1", "one question", false);
    let b = question(&mut s, "w-a2", "another question", false);
    let shot = prepared(&mut s);
    let revision = crate::ask_resolution::check(s.conn(), "crew-a", &a.ask, NOW)
        .unwrap()
        .resolution_revision
        .unwrap();
    ok(&mut s, |tx| {
        crate::ask_resolution::resolve(
            tx,
            crewd_proto::AskResolutionParams {
                crew_id: "crew-a".into(),
                ask: a.ask.clone(),
                expected_resolution_revision: revision,
                disposition: crewd_proto::AskResolutionDisposition::Answer,
                text: "explicit original Human answer".into(),
            },
        )
    });
    let after_answer = business(&s);
    ok(&mut s, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 1.0,
                suggested: true,
            },
        )
    });
    assert!(listing(&s, "crew-a").items.is_empty());
    assert_eq!(after_answer, business(&s));
    assert_eq!(asks::get(s.conn(), "crew-a", &b.ask).unwrap(), b);

    let mut s = fixture();
    question(&mut s, "w-a1", "time bounded", true);
    question(&mut s, "w-a2", "still open", false);
    let shot = prepared(&mut s);
    ok_at(&mut s, NOW + 60_000, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 1.0,
                suggested: true,
            },
        )
    });
    assert_eq!(db::get(s.conn(), &shot.id).unwrap().unwrap().phase, "stale");
}

#[test]
fn lost_provider_response_recovers_unknown_and_never_reenters_dispatch() {
    let mut s = fixture();
    question(&mut s, "w-a1", "first", false);
    question(&mut s, "w-a2", "second", false);
    let shot = prepared(&mut s);
    let before = business(&s);
    assert_eq!(ok(&mut s, recover_page), 1);
    assert_eq!(ok(&mut s, recover_page), 0);
    for _ in 0..12 {
        ok(&mut s, observe_page);
    }
    assert!(ok(&mut s, next_dispatch).is_none());
    assert_eq!(
        count(&s, "SELECT count(*) FROM admin_ask_group_observations"),
        1
    );
    assert_eq!(listing(&s, "crew-a").items[0].state, State::Unknown);
    ok(&mut s, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 0.9,
                suggested: true,
            },
        )
    });
    assert_eq!(listing(&s, "crew-a").items[0].state, State::Unknown);
    assert_eq!(before, business(&s));
}

#[test]
fn actual_restore_witness_and_config_disable_do_not_reanimate_old_attempts() {
    let mut s = fixture();
    question(&mut s, "w-a1", "first", false);
    question(&mut s, "w-a2", "second", false);
    let shot = prepared(&mut s);
    let before = business(&s);
    ok(&mut s, |tx| {
        crewd_store::meta_set(
            tx.conn(),
            "personalDraft.restoreStamp.v1",
            &uuid::Uuid::new_v4().to_string(),
        )?;
        reconcile_cursor(tx)
    });
    ok(&mut s, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 1.0,
                suggested: true,
            },
        )
    });
    assert!(listing(&s, "crew-a").items.is_empty());
    for _ in 0..8 {
        ok(&mut s, observe_page);
    }
    assert!(ok(&mut s, next_dispatch).is_none());
    assert_eq!(before, business(&s));
    configure(&mut s, false);
    let before = count(&s, "SELECT count(*) FROM admin_ask_group_observations");
    for _ in 0..8 {
        ok(&mut s, observe_page);
    }
    assert!(ok(&mut s, next_dispatch).is_none());
    assert_eq!(
        before,
        count(&s, "SELECT count(*) FROM admin_ask_group_observations")
    );
    assert_eq!(listing(&s, "crew-a").availability, Availability::Disabled);
}

#[test]
fn restored_history_stays_excluded_when_new_questions_arrive_until_explicit_opt_in() {
    let mut s = fixture();
    let old_a = question(&mut s, "w-a1", "before restore A", false);
    let old_b = question(&mut s, "w-a2", "before restore B", false);
    let _old_attempt = prepared(&mut s);
    let old_settings = judgments::configuration(s.conn()).unwrap().revision;
    ok(&mut s, |tx| {
        crewd_store::meta_set(
            tx.conn(),
            "personalDraft.restoreStamp.v1",
            &uuid::Uuid::new_v4().to_string(),
        )?;
        reconcile_cursor(tx)
    });
    let floor = db::cursor(s.conn(), "crew-a").unwrap().unwrap().floor;
    assert_eq!(floor, old_b.ask[1..].parse::<i64>().unwrap());
    let new_a = question(&mut s, "w-a1", "after restore A", false);
    let before = business(&s);
    for _ in 0..32 {
        ok(&mut s, observe_page);
        assert!(
            ok(&mut s, next_dispatch).is_none(),
            "one new question must not enroll a restored historical peer"
        );
    }
    assert_eq!(before, business(&s));

    let new_b = question(&mut s, "w-a2", "after restore B", false);
    let before = business(&s);
    let shot = prepared(&mut s);
    assert_eq!(shot.first.id, new_a.ask);
    assert_eq!(shot.second.id, new_b.ask);
    ok(&mut s, |tx| settle(tx, &shot, Outcome::Unknown));
    for _ in 0..32 {
        ok(&mut s, observe_page);
        assert!(ok(&mut s, next_dispatch).is_none());
    }
    let epoch = scope(s.conn()).unwrap().restore_epoch;
    let historical_pairs: i64 = s.conn().query_row(
        "SELECT count(*) FROM admin_ask_group_observations WHERE restore_stamp=?1 AND (first_number<=?2 OR second_number<=?2)",
        params![epoch, floor], |r| r.get(0),
    ).unwrap();
    assert_eq!(historical_pairs, 0);
    assert_eq!(before, business(&s));

    // Only a new explicit settings UUID can opt historical questions back in.
    configure(&mut s, true);
    assert_ne!(
        judgments::configuration(s.conn()).unwrap().revision,
        old_settings
    );
    let before = business(&s);
    let mut old_pair_reenrolled = false;
    for _ in 0..64 {
        ok(&mut s, observe_page);
        if let Some(shot) = ok(&mut s, next_dispatch) {
            old_pair_reenrolled |= shot.first.id == old_a.ask && shot.second.id == old_b.ask;
            ok(&mut s, |tx| settle(tx, &shot, Outcome::Unknown));
        }
    }
    assert!(
        old_pair_reenrolled,
        "explicit opt-in must still permit the original pair"
    );
    assert_eq!(before, business(&s));
}

#[test]
fn structured_and_human_authored_questions_are_not_free_text_merge_candidates() {
    let mut s = fixture();
    let ordinary = question(&mut s, "w-a1", "ordinary open question", false);
    let human = ok(&mut s, |tx| {
        todo_attributes::create_human(
            tx,
            &Sender::Human,
            crewd_proto::HumanTaskCreateParams {
                request_id: uuid::Uuid::new_v4().to_string(),
                crew_id: "crew-a".into(),
                channel_id: crewd_store::channels::lobby_id("crew-a"),
                text: "explicit Human task".into(),
            },
        )
    });
    let structured = ok(&mut s, |tx| {
        let scope = crate::managed_objects::scope(tx.conn())?;
        crate::decision_asks::open(
            tx,
            "w-a1",
            crewd_proto::decision_asks::Open {
                scope,
                request_id: uuid::Uuid::new_v4().to_string(),
                draft: crewd_proto::decision_asks::Draft {
                    question: "Select publication route".into(),
                    context: "synthetic explicit options".into(),
                    options: vec![
                        crewd_proto::decision_asks::OptionDraft {
                            label: "Publish".into(),
                        },
                        crewd_proto::decision_asks::OptionDraft {
                            label: "Wait".into(),
                        },
                    ],
                    about: vec![crewd_proto::managed_objects::DecisionAbout::Crew {
                        id: "crew-a".into(),
                    }],
                    default: None,
                    reversibility: None,
                },
            },
        )
    })
    .0;
    assert!(
        source(s.conn(), ordinary.ask[1..].parse().unwrap(), NOW)
            .unwrap()
            .is_some()
    );
    assert!(
        source(s.conn(), human.ask.ask[1..].parse().unwrap(), NOW)
            .unwrap()
            .is_none()
    );
    assert!(
        source(s.conn(), structured.opened.ask[1..].parse().unwrap(), NOW)
            .unwrap()
            .is_none()
    );
    for _ in 0..12 {
        ok(&mut s, observe_page);
    }
    assert!(ok(&mut s, next_dispatch).is_none());
}

#[test]
fn source_removal_archival_and_agent_reads_cannot_publish_private_group_content() {
    let mut s = fixture();
    let a = question(&mut s, "w-a1", "synthetic question A", false);
    question(&mut s, "w-a2", "synthetic question B", false);
    let shot = prepared(&mut s);
    assert!(
        list(
            s.conn(),
            &Sender::Session("w-a1".into()),
            List {
                crew_id: "crew-a".into(),
                after: None,
                limit: None
            },
            NOW
        )
        .is_err()
    );
    ok(&mut s, |tx| {
        tx.conn().execute(
            "DELETE FROM messages WHERE id=?1",
            [&shot.first.source_message_id],
        )?;
        Ok(())
    });
    ok(&mut s, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 1.0,
                suggested: true,
            },
        )
    });
    assert!(listing(&s, "crew-a").items.is_empty());
    assert_eq!(
        asks::get(s.conn(), "crew-a", &a.ask).unwrap().question,
        a.question
    );
    // The real fixture has a live child. Use the authoritative child-first
    // archive path; never bypass CrewLiveChildren for this optional surface.
    ok(&mut s, |tx| crews::archive(tx, "crew-b"));
    assert!(crews::is_archived(s.conn(), "crew-b").unwrap());
    ok(&mut s, |tx| crews::archive(tx, "crew-a"));
    assert_eq!(listing(&s, "crew-a").availability, Availability::Archived);
}

#[test]
fn foreign_crew_never_pairs_and_real_fk_purge_keeps_neighbors() {
    let mut s = fixture();
    question(&mut s, "cap-x", "X1", false);
    question(&mut s, "cap-x", "X2", false);
    question(&mut s, "w-a1", "A1", false);
    question(&mut s, "w-a2", "A2", false);
    for _ in 0..16 {
        ok(&mut s, observe_page);
    }
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM admin_ask_group_observations WHERE crew_id='crew-x'"
        ),
        1
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM admin_ask_group_observations WHERE crew_id='crew-a'"
        ),
        1
    );
    ok(&mut s, |tx| crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crews::purge(tx, "crew-x"));
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM admin_ask_group_observations WHERE crew_id='crew-x'"
        ),
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM admin_ask_group_observations WHERE crew_id='crew-a'"
        ),
        1
    );
}

#[test]
fn ddl_failure_rolls_back_original_version_tables_and_source() {
    let mut s = crate::testkit::v31_store();
    let old: i64 = s
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    s.conn().execute_batch("CREATE TABLE admin_ask_group_scan(retained TEXT);INSERT INTO admin_ask_group_scan VALUES('original conflict');").unwrap();
    assert!(
        crate::run(&mut s, NOW, |tx| {
            db::migrate(tx.conn())?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(
        s.conn()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        old
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM sqlite_master WHERE name='admin_ask_group_observations'"
        ),
        0
    );
    assert_eq!(
        s.conn()
            .query_row("SELECT retained FROM admin_ask_group_scan", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "original conflict"
    );
}

#[test]
fn all_twenty_current_questions_reach_the_oldest_pair_without_a_recent_window_or_dense_queue() {
    let mut s = fixture();
    let ids = (0..20)
        .map(|n| {
            question(
                &mut s,
                "w-a1",
                &format!("synthetic current question {n}"),
                false,
            )
            .ask
        })
        .collect::<Vec<_>>();
    let before = business(&s);
    let mut complete = false;
    let mut dispatched = 0;
    for _ in 0..500 {
        let old = count(&s, "SELECT count(*) FROM admin_ask_group_observations");
        ok(&mut s, observe_page);
        assert!(
            count(&s, "SELECT count(*) FROM admin_ask_group_observations") <= old + 1,
            "never pre-generate a dense future dispatch queue"
        );
        if let Some(shot) = ok(&mut s, next_dispatch) {
            dispatched += 1;
            ok(&mut s, |tx| {
                settle(
                    tx,
                    &shot,
                    Outcome::Answered {
                        probability: 0.1,
                        suggested: false,
                    },
                )
            });
        }
        if !listing(&s, "crew-a").partial_coverage {
            complete = true;
            break;
        }
    }
    assert!(complete, "a finite stable scope must actually finish");
    assert_eq!(dispatched, 20 * 19 / 2);
    let first: i64 = ids.first().unwrap()[1..].parse().unwrap();
    let last: i64 = ids.last().unwrap()[1..].parse().unwrap();
    let pair_count:i64=s.conn().query_row("SELECT count(*) FROM admin_ask_group_observations WHERE first_number=?1 AND second_number=?2 AND phase='different'",params![first,last],|r|r.get(0)).unwrap();
    assert_eq!(
        pair_count, 1,
        "the oldest eligible H is not excluded by recentness"
    );
    assert_eq!(before, business(&s));
    let writes = s.conn().total_changes();
    for _ in 0..10 {
        ok(&mut s, observe_page);
        assert!(ok(&mut s, next_dispatch).is_none());
    }
    assert_eq!(
        s.conn().total_changes(),
        writes,
        "an unchanged completed scope performs no row mutation"
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM admin_ask_group_observations"),
        20 * 19 / 2
    );
}

#[test]
fn fixed_pass_cut_advances_older_peers_despite_new_questions_and_real_source_changes_require_a_new_round()
 {
    let mut s = fixture();
    let original = (0..4)
        .map(|n| question(&mut s, "w-a1", &format!("original question {n}"), false).ask)
        .collect::<Vec<_>>();
    ok(&mut s, observe_page);
    let cut = db::cursor(s.conn(), "crew-a").unwrap().unwrap().pass_cut;
    for n in 0..18 {
        let before_cut = db::cursor(s.conn(), "crew-a").unwrap().unwrap().pass_cut;
        let new = question(&mut s, "w-a2", &format!("newer question {n}"), false);
        let _ = new;
        assert_eq!(
            db::cursor(s.conn(), "crew-a").unwrap().unwrap().pass_cut,
            before_cut,
            "new published anchors do not restart the active cut"
        );
        ok(&mut s, observe_page);
        if let Some(shot) = ok(&mut s, next_dispatch) {
            ok(&mut s, |tx| {
                settle(
                    tx,
                    &shot,
                    Outcome::Answered {
                        probability: 0.1,
                        suggested: false,
                    },
                )
            });
        }
    }
    let first: i64 = original[0][1..].parse().unwrap();
    let last: i64 = original[3][1..].parse().unwrap();
    assert_eq!(s.conn().query_row("SELECT count(*) FROM admin_ask_group_observations WHERE first_number=?1 AND second_number=?2 AND phase='different'",params![first,last],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert!(
        db::cursor(s.conn(), "crew-a")
            .unwrap()
            .unwrap()
            .completed_cut
            >= cut
    );
    let old = db::cursor(s.conn(), "crew-a").unwrap().unwrap();
    ok(&mut s, |tx| {
        asks::answer(tx, "crew-a", &original[0], "explicit original Human answer")?;
        db::invalidate_coverage(tx.conn())?;
        Ok(())
    });
    let changed = db::cursor(s.conn(), "crew-a").unwrap().unwrap();
    assert_eq!((old.anchor, old.peer), (changed.anchor, changed.peer));
    assert_ne!(old.generation, changed.generation);
    assert!(listing(&s, "crew-a").partial_coverage);
}

#[test]
fn unrelated_judgment_epoch_cannot_requeue_an_unknown_original_but_late_authority_change_rejects_publication()
 {
    let mut s = fixture();
    question(&mut s, "w-a1", "original question A", false);
    question(&mut s, "w-a2", "original question B", false);
    let shot = prepared(&mut s);
    ok(&mut s, |tx| {
        crewd_store::judgment_state::restored(tx.conn())?;
        Ok(())
    });
    assert!(
        !still_current(s.conn(), &shot, NOW).unwrap(),
        "the actual dispatch authority changed"
    );
    ok(&mut s, |tx| {
        settle(
            tx,
            &shot,
            Outcome::Answered {
                probability: 0.99,
                suggested: true,
            },
        )
    });
    assert_eq!(db::get(s.conn(), &shot.id).unwrap().unwrap().phase, "stale");
    for _ in 0..12 {
        ok(&mut s, observe_page);
    }
    assert!(
        ok(&mut s, next_dispatch).is_none(),
        "a transient epoch is not a fresh model intent"
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM admin_ask_group_observations"),
        1
    );
    assert_eq!(listing(&s, "crew-a").items[0].state, State::Stale);
    assert!(listing(&s, "crew-a").partial_coverage);
}
