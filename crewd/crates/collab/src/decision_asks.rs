//! Opted-in H questions publish an organization decision only on actual closure.
use crate::{
    Change, CollabError, ProgressKind, Result, Sender, Tx, asks, crews, managed_objects,
    refusal::{Code, Refusal},
    resource_acl, sessions,
};
use crewd_proto::{
    Ask, AskDefault,
    decision_asks::{Open, OpenLookup, OpenReceipt, View},
    managed_objects::Scope,
};
use crewd_store::{
    StoreError, decision_asks as db, managed_decisions,
    object_refs::{self, ObjectKind},
};
use crewd_work::{AskId, Event};
use rusqlite::Connection;

fn number(id: &str, prefix: char, field: &str) -> Result<i64> {
    id.strip_prefix(prefix)
        .and_then(|digits| digits.parse::<i64>().ok())
        .filter(|n| *n > 0 && id == format!("{prefix}{n}"))
        .ok_or_else(|| resource_acl::invalid(field))
}

fn conflict(request: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ResourceRequestConflict).arg("request", request))
}

fn forbidden() -> CollabError {
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}

fn request_id(request: &str) -> Result<()> {
    if !crewd_proto::valid_remote_uuid(request) {
        return Err(resource_acl::invalid("requestId"));
    }
    Ok(())
}

pub fn open(tx: &mut Tx<'_>, session: &str, request: Open) -> Result<(OpenReceipt, bool)> {
    request_id(&request.request_id)?;
    let c = tx.conn();
    let crew = sessions::get(c, session)?.crew_id;
    resource_acl::actor(c, &Sender::Session(session.into()), &crew, true)?;
    if managed_objects::scope(c)? != request.scope {
        return Err(conflict(&request.request_id));
    }
    let digest = resource_acl::digest(&request)?;
    if let Some((prior, receipt)) = db::receipt(c, &crew, session, &request.request_id)? {
        return if prior.as_slice() == digest && receipt.scope == request.scope {
            Ok((receipt, false))
        } else {
            Err(conflict(&request.request_id))
        };
    }
    if !db::draft_valid(&request.draft) {
        return Err(resource_acl::invalid("draft"));
    }
    let example = db::decision_draft(&request.draft, "A")?;
    managed_objects::check_decision_draft(c, &example)?;
    let prompts = crate::prompts::render_file(crate::settings::locale(c)?);
    let notice = prompts.get("notice.ask.decision_capture");
    let mut announcement = format!("{}\n\n{}", db::announcement(&request.draft), notice.trim());
    if let Some(default) = &request.draft.default {
        let minutes = default.after_minutes.to_string();
        let fallback = crate::prompts::fill(
            prompts.get("notice.ask.decision_default"),
            &[("option", &default.option_id), ("minutes", &minutes)],
        );
        announcement.push('\n');
        announcement.push_str(fallback.trim());
    }
    let ask = asks::open_with_message(
        tx,
        session,
        asks::OpenAsk {
            question: request.draft.question.clone(),
            default: request.draft.default.as_ref().map(|choice| AskDefault {
                text: choice.option_id.clone(),
                after_minutes: choice.after_minutes as i64,
            }),
            resume_note: None,
        },
        Some(announcement),
    )?;
    let ask_number = number(&ask.ask, 'H', "ask")?;
    let opened = db::insert(tx.conn(), ask_number, &crew, &request.draft)?;
    let receipt = OpenReceipt {
        scope: request.scope,
        request_id: request.request_id,
        opened,
    };
    db::record(tx.conn(), ask_number, session, &digest, &receipt)?;
    sessions::note_progress(tx, session, ProgressKind::Ontology)?;
    Ok((receipt, true))
}

pub fn lookup(c: &Connection, session: &str, scope: &Scope, request: &str) -> Result<OpenLookup> {
    request_id(request)?;
    if !crewd_proto::valid_remote_uuid(&scope.restore_epoch) {
        return Err(resource_acl::invalid("restoreEpoch"));
    }
    let crew = sessions::get(c, session)?.crew_id;
    resource_acl::actor(c, &Sender::Session(session.into()), &crew, false)?;
    let live = managed_objects::scope(c)?;
    if live.db_id != scope.db_id {
        return Err(forbidden());
    }
    let stored = db::receipt(c, &crew, session, request)?.map(|(_, receipt)| receipt);
    if scope.restore_epoch != live.restore_epoch {
        return Ok(OpenLookup::Historical {
            receipt: stored.filter(|receipt| receipt.scope == *scope),
        });
    }
    Ok(match stored {
        Some(receipt) if receipt.scope == *scope => OpenLookup::Known { receipt },
        Some(_) => OpenLookup::Historical { receipt: None },
        None => OpenLookup::NotFound,
    })
}

pub fn get(c: &Connection, crew: &str, ask: &str) -> Result<Option<View>> {
    let number = number(ask, 'H', "ask")?;
    Ok(db::read_ask(c, number)?.filter(|view| view.crew_id == crew))
}

pub fn origin(c: &Connection, decision_id: &str) -> Result<Option<View>> {
    db::read_decision(c, number(decision_id, 'D', "decisionId")?).map_err(Into::into)
}

/// Raw Ask answers accept the same historical aliases as the existing ledger.
pub fn answer_is_structured(c: &Connection, crew: &str, ask: &str) -> Result<bool> {
    let reference = asks::ask_reference(c, crew, ask)?;
    Ok(db::read_ask(c, reference.number)?.is_some())
}

pub fn reply_is_structured(c: &Connection, channel: &str, reply: &str) -> Result<bool> {
    let answering = crate::messages::resolve_reply(c, &Sender::Human, channel, reply)?.answering;
    match answering {
        Some((crew, ask)) => Ok(get(c, &crew, &ask)?.is_some()),
        None => Ok(false),
    }
}

pub(crate) fn validate_answer(c: &Connection, number: i64, text: &str) -> Result<()> {
    if let Some(view) = db::read_ask(c, number)? {
        crews::refuse_archived(c, &view.crew_id)?;
        if view.capture.is_none() && db::parse_human(&view.draft, text).is_none() {
            return Err(CollabError::Invalid(Refusal::new(
                Code::AskDecisionOptionRequired,
            )));
        }
    }
    Ok(())
}

pub(crate) fn capture(tx: &mut Tx<'_>, ask: &Ask, core: AskId, events: &[Event]) -> Result<()> {
    let number = number(&ask.ask, 'H', "ask")?;
    let Some(view) = db::read_ask(tx.conn(), number)? else {
        return Ok(());
    };
    if view.crew_id != ask.crew_id || view.capture.is_some() {
        return Err(StoreError::Invalid(
            "structured decision question closure scope mismatch".into(),
        )
        .into());
    }
    let reference = object_refs::get(tx.conn(), ObjectKind::HumanTask, number)?
        .filter(|r| r.crew_id == ask.crew_id)
        .ok_or_else(|| {
            StoreError::Invalid("structured decision question reference missing".into())
        })?;
    if crewd_store::work_organization::core_for_reference(
        tx.conn(),
        ObjectKind::HumanTask,
        reference.number,
    )?
    .as_deref()
        != Some(core.to_string().as_str())
    {
        return Err(StoreError::Invalid(
            "structured decision question WorkBook identity mismatch".into(),
        )
        .into());
    }
    let mut terminal = events
        .iter()
        .filter(|event| db::terminal_choice(&view.draft, core, event).is_some());
    let event = terminal
        .next()
        .filter(|_| terminal.next().is_none())
        .ok_or_else(|| {
            StoreError::Invalid("structured decision question requires one terminal event".into())
        })?;
    let decision = managed_decisions::record_captured(tx.conn(), number, core, event)?;
    tx.note(Change::OntologyChanged {
        crew_id: ask.crew_id.clone(),
        object_ids: vec![decision.id],
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, ok, ok_at, store};
    use crewd_proto::{
        AskResolutionDisposition, AskResolutionParams, AskStateView,
        decision_asks::{DefaultChoice, Draft, OptionDraft, Source},
        managed_objects::{DecisionAbout, Object},
    };
    use crewd_store::Store;

    fn request(s: &Store, default: bool) -> Open {
        Open {
            scope: managed_objects::scope(s.conn()).unwrap(),
            request_id: uuid::Uuid::new_v4().to_string(),
            draft: Draft {
                question: "如何切换？".into(),
                context: "需要保证切换时只能从一个位置写入。".into(),
                options: ["停止旧服务", "继续双写"]
                    .into_iter()
                    .map(|label| OptionDraft {
                        label: label.into(),
                    })
                    .collect(),
                about: vec![DecisionAbout::Crew {
                    id: "crew-a".into(),
                }],
                default: default.then(|| DefaultChoice {
                    option_id: "A".into(),
                    after_minutes: 1,
                }),
                reversibility: None,
            },
        }
    }

    #[test]
    fn structured_open_exact_replay_ordinary_question_and_authorized_group_reply() {
        let mut s = store();
        let req = request(&s, false);
        let ((receipt, fresh), facts) = ok_at(&mut s, NOW, |tx| open(tx, "w-a1", req.clone()));
        assert!(fresh);
        assert_eq!(
            facts
                .iter()
                .filter(|f| matches!(f, Change::Progress {
            session_id, kind: ProgressKind::Ontology,
        } if session_id == "w-a1"))
                .count(),
            1
        );
        let ask = &receipt.opened.ask;
        let original = asks::get(s.conn(), "crew-a", ask).unwrap();
        let message_id = original.message_id.as_deref().unwrap();
        let message = crate::messages::get(s.conn(), message_id).unwrap();
        assert!(message.text.contains("A 停止旧服务"));
        assert!(reply_is_structured(s.conn(), &message.channel_id, message_id).unwrap());
        let ((again, fresh), facts) = ok_at(&mut s, NOW + 1, |tx| open(tx, "w-a1", req.clone()));
        assert!(!fresh && facts.is_empty());
        assert_eq!(again, receipt);
        assert_eq!(
            lookup(s.conn(), "w-a2", &req.scope, &req.request_id).unwrap(),
            OpenLookup::NotFound
        );
        let mut tampered = req.clone();
        tampered.draft.context.push('。');
        assert!(
            matches!(crate::run(&mut s, NOW, |tx| open(tx, "w-a1", tampered)),
            Err(CollabError::Conflict(r)) if r.code == Code::ResourceRequestConflict)
        );

        let reply = |text: &str| crate::messages::NewPost {
            channel_id: message.channel_id.clone(),
            text: text.into(),
            in_reply_to: Some(message_id.into()),
            ..Default::default()
        };
        let messages = count(&s, "SELECT count(*) FROM messages");
        let wakes = count(&s, "SELECT count(*) FROM wakes");
        assert!(matches!(crate::run(&mut s, NOW + 2, |tx|
            crate::messages::post(tx, Sender::Human, reply("A or B"))),
            Err(CollabError::Invalid(r)) if r.code == Code::AskDecisionOptionRequired));
        assert_eq!(count(&s, "SELECT count(*) FROM messages"), messages);
        assert_eq!(count(&s, "SELECT count(*) FROM wakes"), wakes);
        assert_eq!(
            asks::get(s.conn(), "crew-a", ask).unwrap().state,
            AskStateView::Open
        );

        let (_, facts) = ok_at(&mut s, NOW + 3, |tx| {
            crate::messages::post(tx, Sender::Human, reply("B\n需要保留原路径"))
        });
        let result = get(s.conn(), "crew-a", ask).unwrap().unwrap();
        let captured = result.capture.as_ref().unwrap();
        assert_eq!(captured.source, Source::Human);
        assert_eq!(captured.option_id, "B");
        assert_eq!(captured.human_reason.as_deref(), Some("需要保留原路径"));
        assert_eq!(
            origin(s.conn(), &captured.decision_id).unwrap(),
            Some(result.clone())
        );
        assert_eq!(
            facts
                .iter()
                .filter(
                    |f| matches!(f, Change::OntologyChanged { crew_id, object_ids }
            if crew_id == "crew-a" && object_ids == &[captured.decision_id.clone()])
                )
                .count(),
            1
        );
        assert!(!facts.iter().any(|f| matches!(f, Change::Progress { .. })));
        assert_eq!(count(&s, "SELECT count(*) FROM messages"), messages + 1);
        assert_eq!(count(&s, "SELECT count(*) FROM wakes"), wakes + 1);
        assert_eq!(
            lookup(s.conn(), "w-a1", &req.scope, &req.request_id).unwrap(),
            OpenLookup::Known { receipt }
        );
        assert!(!reply_is_structured(s.conn(), &message.channel_id, message_id).unwrap());
        ok(&mut s, |tx| {
            crate::messages::post(tx, Sender::Human, reply("改主意了"))
        });
        assert_eq!(count(&s, "SELECT count(*) FROM managed_decisions"), 1);

        let ordinary = ok(&mut s, |tx| {
            asks::open(
                tx,
                "w-a1",
                asks::OpenAsk {
                    question: "普通问题".into(),
                    default: None,
                    resume_note: None,
                },
            )
        });
        ok(&mut s, |tx| {
            asks::answer(tx, "crew-a", &ordinary.ask, "普通自由文字")
        });
        assert!(get(s.conn(), "crew-a", &ordinary.ask).unwrap().is_none());
        assert_eq!(count(&s, "SELECT count(*) FROM managed_decisions"), 1);
    }

    #[test]
    fn structured_default_is_system_only_and_legacy_page_filters_before_keyset() {
        let mut s = store();
        let req = request(&s, true);
        let (original, _) = ok(&mut s, |tx| open(tx, "w-a1", req.clone()));
        let manual = ok(&mut s, |tx| {
            managed_objects::submit(
                tx,
                managed_objects::Actor::Human("local"),
                crewd_proto::managed_objects::Submit {
                    scope: req.scope.clone(),
                    crew_id: "crew-a".into(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    change: crewd_proto::managed_objects::Change::RecordDecision {
                        draft: db::decision_draft(&req.draft, "B")?,
                    },
                },
            )
        });
        let Object::Decision { decision: manual } = manual.object else {
            panic!("decision")
        };
        let h = &original.opened.ask;
        let (_, early) = ok_at(&mut s, NOW + MIN - 1, asks::apply_all_due_defaults);
        assert!(early.is_empty());
        let (changed, facts) = ok_at(&mut s, NOW + MIN, asks::apply_all_due_defaults);
        assert_eq!(changed, [h.clone()]);
        let captured = get(s.conn(), "crew-a", h)
            .unwrap()
            .unwrap()
            .capture
            .unwrap();
        assert_eq!(captured.source, Source::SystemDefault);
        assert_eq!(captured.option_id, "A");
        assert_eq!(captured.human_reason, None);
        let rows = crate::wakes::for_source(
            s.conn(),
            crate::wakes::WakeKind::AskAnswer,
            &crate::wakes::source::ask("crew-a", h),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        let batch =
            crate::render::build_batch(s.conn(), "w-a1", crate::Locale::Zh, &rows, NOW + MIN)
                .unwrap();
        assert!(
            crate::render::wake_text(crate::Locale::Zh, &batch)
                .contains(&format!("A 停止旧服务 ({})", captured.decision_id))
        );
        assert!(
            facts
                .iter()
                .any(|f| matches!(f, Change::OntologyChanged { object_ids, .. }
            if object_ids == &[captured.decision_id.clone()]))
        );
        assert!(
            !facts
                .iter()
                .any(|f| matches!(f, Change::Progress { .. } | Change::MessageAppended { .. }))
        );
        let who = managed_objects::Actor::Session("w-b1");
        let Object::Decision { decision } =
            managed_objects::get_with_defaults(s.conn(), who, &captured.decision_id, true).unwrap()
        else {
            panic!("decision")
        };
        assert_eq!(decision.author.kind, "systemDefault");
        assert!(decision.author.session_id.is_none());
        assert_eq!(decision.draft.choice, "停止旧服务");
        assert_eq!(
            decision.created_at,
            asks::get(s.conn(), "crew-a", h).unwrap().closed_at.unwrap()
        );
        assert!(managed_objects::get(s.conn(), who, &captured.decision_id).is_err());
        assert_eq!(
            managed_objects::decisions(s.conn(), who, None)
                .unwrap()
                .items,
            [manual.clone()]
        );
        assert_eq!(
            managed_objects::decisions_with_defaults(s.conn(), who, None, true)
                .unwrap()
                .items,
            [decision.clone(), manual.clone()]
        );
        assert_eq!(
            managed_objects::decisions_with_defaults(s.conn(), who, Some(&decision.id), true)
                .unwrap()
                .items,
            [manual]
        );
        assert!(crate::run(&mut s, NOW + MIN, |tx| asks::answer(tx, "crew-a", h, "B")).is_err());
    }

    #[test]
    fn failure_rolls_back_question_open_or_typed_answer_with_no_decision_side_effect() {
        let mut s = store();
        let req = request(&s, true);
        s.conn()
            .execute_batch(
                "CREATE TRIGGER fail_open BEFORE INSERT ON decision_ask_open_receipts
            BEGIN SELECT RAISE(ABORT,'synthetic receipt fault'); END;",
            )
            .unwrap();
        assert!(crate::run(&mut s, NOW, |tx| open(tx, "w-a1", req.clone())).is_err());
        assert_eq!(count(&s, "SELECT count(*) FROM decision_asks"), 0);
        assert_eq!(count(&s, "SELECT count(*) FROM messages"), 0);
        assert!(
            sessions::get(s.conn(), "w-a1")
                .unwrap()
                .last_progress_at
                .is_none()
        );
        s.conn().execute_batch("DROP TRIGGER fail_open").unwrap();
        let (opened, _) = ok(&mut s, |tx| open(tx, "w-a1", req));
        let h = opened.opened.ask;
        let rev = crate::ask_resolution::check(s.conn(), "crew-a", &h, NOW)
            .unwrap()
            .resolution_revision
            .unwrap();
        let answer = AskResolutionParams {
            crew_id: "crew-a".into(),
            ask: h.clone(),
            expected_resolution_revision: rev,
            disposition: AskResolutionDisposition::Answer,
            text: "B\n选择理由".into(),
        };
        s.conn()
            .execute_batch(
                "CREATE TRIGGER fail_capture BEFORE UPDATE OF decision_number ON decision_asks
            BEGIN SELECT RAISE(ABORT,'synthetic capture fault'); END;",
            )
            .unwrap();
        let wakes = count(&s, "SELECT count(*) FROM wakes");
        assert!(
            crate::run(&mut s, NOW + 1, |tx| crate::ask_resolution::resolve(
                tx,
                answer.clone()
            ))
            .is_err()
        );
        assert!(crate::run(&mut s, NOW + MIN, asks::apply_all_due_defaults).is_err());
        assert_eq!(
            asks::get(s.conn(), "crew-a", &h).unwrap().state,
            AskStateView::Open
        );
        assert!(
            get(s.conn(), "crew-a", &h)
                .unwrap()
                .unwrap()
                .capture
                .is_none()
        );
        assert_eq!(count(&s, "SELECT count(*) FROM managed_decisions"), 0);
        assert_eq!(count(&s, "SELECT count(*) FROM wakes"), wakes);
        s.conn().execute_batch("DROP TRIGGER fail_capture").unwrap();
        let done = ok(&mut s, |tx| crate::ask_resolution::resolve(tx, answer));
        let capture = get(s.conn(), "crew-a", &h)
            .unwrap()
            .unwrap()
            .capture
            .unwrap();
        assert_eq!(capture.event_seq, done.event_seq);
        assert_eq!(capture.source, Source::Human);
        assert_eq!(
            ok_at(&mut s, NOW + MIN, asks::apply_all_due_defaults).0,
            Vec::<String>::new()
        );
    }

    #[test]
    fn original_open_is_historical_after_restore_stamp_and_cannot_replay() {
        let mut s = store();
        let original = request(&s, true);
        let (receipt, _) = ok(&mut s, |tx| open(tx, "w-a1", original.clone()));
        let before = asks::resolution_subject(s.conn(), "crew-a", &receipt.opened.ask, None)
            .unwrap()
            .revision
            .unwrap();
        let prior = lookup(s.conn(), "w-a1", &original.scope, &original.request_id).unwrap();
        assert_eq!(
            prior,
            OpenLookup::Known {
                receipt: receipt.clone()
            }
        );
        crewd_store::meta_set(
            s.conn(),
            "personalDraft.restoreStamp.v1",
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        let current = managed_objects::scope(s.conn()).unwrap();
        assert_ne!(current, original.scope);
        let after = asks::resolution_subject(s.conn(), "crew-a", &receipt.opened.ask, None)
            .unwrap()
            .revision
            .unwrap();
        assert_ne!(before, after);
        assert_eq!(
            lookup(s.conn(), "w-a1", &original.scope, &original.request_id).unwrap(),
            OpenLookup::Historical {
                receipt: Some(receipt)
            }
        );
        assert_eq!(
            lookup(s.conn(), "w-a1", &current, &original.request_id).unwrap(),
            OpenLookup::Historical { receipt: None }
        );
        assert!(
            matches!(crate::run(&mut s, NOW + 1, |tx| open(tx, "w-a1", original.clone())),
            Err(CollabError::Conflict(r)) if r.code == Code::ResourceRequestConflict)
        );
        let same_id = Open {
            scope: current,
            ..original
        };
        assert!(
            matches!(crate::run(&mut s, NOW + 2, |tx| open(tx, "w-a1", same_id)),
            Err(CollabError::Conflict(r)) if r.code == Code::ResourceRequestConflict)
        );
        assert_eq!(count(&s, "SELECT count(*) FROM decision_asks"), 1);
        assert_eq!(count(&s, "SELECT count(*) FROM messages"), 1);
    }

    #[test]
    fn schema_25_upgrade_has_no_automatic_capture_and_keeps_original_ask() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open_data_dir(dir.path()).unwrap();
        s.conn()
            .execute(
                "INSERT INTO crews(id,title,created_at,updated_at) VALUES('crew-a','A','t','t')",
                [],
            )
            .unwrap();
        let original = ok(&mut s, |tx| {
            let core = asks::with_book(tx, "crew-a", |book, now| {
                book.create_human_task(
                    &crewd_work::Actor::Human,
                    now,
                    crewd_work::CrewId::new("crew-a"),
                    "legacy manual",
                )
            })?
            .value;
            asks::get(
                tx.conn(),
                "crew-a",
                &asks::public_ask(tx.conn(), core)?.id(),
            )
        });
        s.conn().execute_batch(
            "DROP TABLE public_command_purge_tombstones;
             DROP TABLE decision_ask_open_receipts;
             DROP TABLE decision_asks;
             PRAGMA user_version=25;",
        ).unwrap();
        drop(s);
        let s = Store::open_data_dir(dir.path()).unwrap();
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            26
        );
        assert_eq!(
            asks::get(s.conn(), "crew-a", &original.ask).unwrap(),
            original
        );
        assert!(get(s.conn(), "crew-a", &original.ask).unwrap().is_none());
        assert_eq!(count(&s, "SELECT count(*) FROM managed_decisions"), 0);
    }
}
