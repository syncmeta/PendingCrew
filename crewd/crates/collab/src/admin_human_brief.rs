//! Same-crew Human checklist assembled from current wait facts and recorded patrol.
use crate::{Result, Sender, admin_cases, asks, crews, patrol_runtime, todo_attributes, work};
use crewd_proto::{
    AdminCaseOutcomePhase, AdminHumanBrief, AdminHumanBriefCase, AdminHumanBriefCaseCoverage,
    AdminHumanBriefPatrol, AdminHumanBriefPatrolState, AdminHumanBriefWait,
    AdminHumanBriefWaitKind, ChannelWorkKind, ChannelWorkObject, ManualTodoState, MemberRef,
    TodoOrigin, WaitTierView,
};
use crewd_store::{
    StoreError,
    object_refs::{ObjectKind, ObjectReference},
};
use crewd_work::{AskState, CrewId, HumanAsk, HumanWait, Ts, WaitTier};
use rusqlite::{Connection, OptionalExtension, params};
use std::cmp::Reverse;

const HUMAN_ITEMS: usize = 20;
const WAIT_PROBES: usize = 256;
const BLOCKER_SAMPLE: usize = 12;
const CASE_SAMPLE: usize = 12;
const QUESTION_BYTES: usize = 1024;

fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid Human brief authority".into()).into()
}

pub fn get(c: &Connection, crew: &str, now_ms: i64) -> Result<AdminHumanBrief> {
    crews::refuse_archived(c, crew)?;
    let (human_waits, human_waits_truncated) = waits(c, crew, now_ms)?;
    let (cases, case_coverage) = cases(c, crew, now_ms);
    Ok(AdminHumanBrief {
        crew_id: crew.into(),
        observed_at_ms: now_ms,
        human_waits,
        human_waits_truncated,
        cases,
        case_coverage,
        patrol: patrol(c, crew, now_ms),
    })
}

fn tier(tier: WaitTier) -> WaitTierView {
    match tier {
        WaitTier::Fresh => WaitTierView::Fresh,
        WaitTier::Pinned => WaitTierView::Pinned,
        WaitTier::StillWanted => WaitTierView::StillWanted,
    }
}

fn waits(c: &Connection, crew: &str, now_ms: i64) -> Result<(Vec<AdminHumanBriefWait>, bool)> {
    let book = asks::load_org(c)?;
    let now = Ts(now_ms).max(book.state().clock);
    let scope = CrewId::new(crew);
    let belongs = |id| book.state().item_crew(id) == Some(&scope);
    let mut candidates: Vec<_> = book
        .human_waits(now)
        .into_iter()
        .filter_map(|wait| {
            if book.state().ask_crew(wait.ask) != Some(&scope)
                || wait.default_due_at.is_some_and(|due| due <= now)
            {
                return None;
            }
            let ask = book.ask(wait.ask)?;
            let has_local_blocker = wait.items.iter().copied().any(belongs);
            (!ask.human_task && (ask.standalone || has_local_blocker)).then_some((
                wait,
                ask,
                has_local_blocker,
            ))
        })
        .collect();
    // Human-authored H items intentionally never participate in Ask escalation.
    candidates.extend(
        book.state()
            .asks
            .values()
            .filter(|ask| {
                ask.human_task
                    && ask.state == AskState::Open
                    && book.state().ask_crew(ask.id) == Some(&scope)
            })
            .map(|ask| {
                (
                    HumanWait {
                        ask: ask.id,
                        items: Vec::new(),
                        owners: Vec::new(),
                        waited_ms: now.since(ask.asked_at).max(0),
                        tier: WaitTier::Fresh,
                        default_due_at: None,
                    },
                    ask,
                    false,
                )
            }),
    );
    candidates.sort_by_key(|(wait, _, blocks)| {
        (
            !(*blocks && wait.tier != WaitTier::Fresh),
            Reverse(wait.waited_ms),
            wait.ask,
        )
    });
    let mut truncated = candidates.len() > WAIT_PROBES;
    let mut rows = Vec::new();
    let home = crewd_store::channels::human_id(&crews::home_of(c, crew)?);
    for (wait, ask, _) in candidates.into_iter().take(WAIT_PROBES) {
        let reference = asks::public_ask(c, ask.id)?;
        if reference.crew_id != crew {
            return Err(invalid());
        }
        let object = ChannelWorkObject {
            kind: ChannelWorkKind::HumanTask,
            id: reference.id(),
        };
        let attrs = todo_attributes::get(c, &Sender::Human, &object)?;
        let kind = if ask.human_task && attrs.origin == TodoOrigin::HumanTask {
            AdminHumanBriefWaitKind::HumanTask
        } else if !ask.human_task && attrs.origin == TodoOrigin::AgentAsk {
            AdminHumanBriefWaitKind::Question
        } else {
            return Err(invalid());
        };
        if attrs.manual_state == Some(ManualTodoState::Done)
            || (ask.human_task && matches!(attrs.owner, Some(MemberRef::Session { .. })))
        {
            continue;
        }
        if rows.len() == HUMAN_ITEMS {
            truncated = true;
            break;
        }
        let mut blockers: Vec<_> = wait
            .items
            .into_iter()
            .filter(|id| belongs(*id))
            .take(BLOCKER_SAMPLE + 1)
            .collect();
        let blockers_truncated = blockers.len() > BLOCKER_SAMPLE;
        blockers.truncate(BLOCKER_SAMPLE);
        let blocking_work_ids = blockers
            .into_iter()
            .map(|id| work::reference(c, ObjectKind::Plan, crew, id))
            .collect::<Result<_>>()?;
        let source_message_id = source_message(c, &reference, &home, ask)?;
        let preview_end = ask.question.floor_char_boundary(QUESTION_BYTES);
        rows.push(AdminHumanBriefWait {
            ask_id: object.id,
            kind,
            question: ask.question[..preview_end].into(),
            question_truncated: preview_end < ask.question.len(),
            asked_at_ms: ask.asked_at.0,
            waited_ms: wait.waited_ms,
            tier: tier(wait.tier),
            notified_tier: tier(ask.escalated),
            default_due_at_ms: wait.default_due_at.map(|due| due.0),
            blocking_work_ids,
            blockers_truncated,
            source_channel_id: source_message_id.as_ref().map(|_| home.clone()),
            source_message_id,
        });
    }
    Ok((rows, truncated))
}

fn source_message(
    c: &Connection,
    reference: &ObjectReference,
    home: &str,
    ask: &HumanAsk,
) -> Result<Option<String>> {
    // The message is a jump target, not a second source for the question text.
    // Confirm the original sender and question before exposing the jump.
    Ok(c.query_row(
        "SELECT m.id FROM ask_meta a JOIN messages m ON m.id=a.message_id
         JOIN channels ch ON ch.id=m.channel_id
         WHERE a.crew_id=?1 AND a.ask=?2 AND m.channel_id=?3 AND ch.kind='human'
         AND m.category='ask' AND m.sender_session_id IS ?4 AND m.text=?5",
        params![
            reference.crew_id,
            asks::meta_key(reference),
            home,
            ask.asked_by.session().map(|id| id.as_str()),
            ask.question
        ],
        |r| r.get(0),
    )
    .optional()?)
}

fn cases(
    c: &Connection,
    crew: &str,
    now: i64,
) -> (Vec<AdminHumanBriefCase>, AdminHumanBriefCaseCoverage) {
    use AdminHumanBriefCaseCoverage as Coverage;
    let Ok((cases, more)) = admin_cases::recent_for_human_brief(c, crew, now, CASE_SAMPLE) else {
        return (Vec::new(), Coverage::Unavailable);
    };
    let rows = cases
        .into_iter()
        .filter_map(|case| {
            let outcome_phase = case.outcome_record.map(|v| v.phase);
            if outcome_phase == Some(AdminCaseOutcomePhase::Resolved) {
                return None;
            }
            Some(AdminHumanBriefCase {
                case_id: case.id,
                created_at_ms: case.created_at,
                decision: case.decision,
                execution: case.execution,
                outcome_phase,
                custody_phase: case.custody_record.map(|v| v.phase),
            })
        })
        .collect();
    (
        rows,
        if more {
            Coverage::RecentSample
        } else {
            Coverage::Complete
        },
    )
}

fn patrol(c: &Connection, crew: &str, now: i64) -> AdminHumanBriefPatrol {
    use AdminHumanBriefPatrolState as State;
    let empty = |state| AdminHumanBriefPatrol {
        state,
        run_id: None,
        started_at_ms: None,
        completed_at_ms: None,
        scanned_counts: Default::default(),
        finding_counts: Default::default(),
        findings_truncated: false,
    };
    let run = match patrol_runtime::human_observation(c, crew) {
        Ok(Some(run))
            if uuid::Uuid::parse_str(&run.run_id).is_ok()
                && run.started_at_ms <= now
                && run
                    .completed_at_ms
                    .is_none_or(|at| (run.started_at_ms..=now).contains(&at)) =>
        {
            run
        }
        Ok(None) => return empty(State::NotRecorded),
        Ok(Some(_)) | Err(_) => return empty(State::Unavailable),
    };
    let scanned_counts = run
        .scanned_counts
        .into_iter()
        .filter(|(key, _)| matches!(key.as_str(), "work" | "entries" | "channels"))
        .collect();
    let finding_counts = run
        .counts
        .into_iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "unowned"
                    | "lost"
                    | "stop_escalated"
                    | "long_waits"
                    | "unclaimed"
                    | "needs_ruling"
                    | "archive_candidates"
                    | "idle_archive_candidates"
                    | "incomplete_channels"
                    | "archive_review_incomplete"
            )
        })
        .collect();
    AdminHumanBriefPatrol {
        state: if run.completed_at_ms.is_some() {
            State::Complete
        } else {
            State::InProgress
        },
        run_id: Some(run.run_id),
        started_at_ms: Some(run.started_at_ms),
        completed_at_ms: run.completed_at_ms,
        scanned_counts,
        finding_counts,
        findings_truncated: run.truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, ok, ok_at, store};
    use crewd_proto::{
        AskDefault, HumanTaskCreateParams, HumanTaskCreateResult, TodoAssignParams,
        TodoSetStateParams, WorkCreateParams,
    };
    use crewd_store::Store;
    use crewd_work::{Actor, BlockOn, Continuation, DefaultOption};

    fn blocked(
        s: &mut Store,
        crew: &str,
        owner: &str,
        default: Option<DefaultOption>,
    ) -> (String, String) {
        ok(s, |tx| {
            let item = work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    owner: Some(owner.into()),
                    title: "waiting work".into(),
                    ..Default::default()
                },
            )?;
            let id = work::local_plan(tx.conn(), crew, &item.id)?;
            let ask = asks::with_book(tx, crew, |book, now| {
                book.block(
                    &Actor::Human,
                    now,
                    id,
                    vec![BlockOn::NewAsk {
                        question: "actual blocking question".into(),
                        default,
                    }],
                    Continuation::Nothing,
                )
                .map(|ap| ap.value.asks[0])
            })?;
            let ids = asks::announce_block_questions(tx, &[ask])?;
            Ok((ids[0].clone(), item.id))
        })
    }

    fn human_task(s: &mut Store, text: &str) -> HumanTaskCreateResult {
        ok(s, |tx| {
            todo_attributes::create_human(
                tx,
                &Sender::Human,
                HumanTaskCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-b".into(),
                    channel_id: "crew-b/lobby".into(),
                    text: text.into(),
                },
            )
        })
    }

    #[test]
    fn live_human_brief_preserves_wait_receipts_and_original_questions_across_reopen() {
        let mut s = store();
        let (blocking, work) = blocked(&mut s, "crew-b", "w-b1", None);
        let (due_default, _) = blocked(
            &mut s,
            "crew-b",
            "w-b1",
            Some(DefaultOption {
                text: "existing default".into(),
                after_ms: MIN,
            }),
        );
        let sibling = ok(&mut s, |tx| {
            asks::open(
                tx,
                "w-a1",
                asks::OpenAsk {
                    question: "separate crew in the same Human conversation".into(),
                    default: None,
                    resume_note: None,
                },
            )
        });
        let future_default = ok(&mut s, |tx| {
            asks::open(
                tx,
                "w-b1",
                asks::OpenAsk {
                    question: "still waiting for Human before the default".into(),
                    default: Some(AskDefault {
                        text: "continue".into(),
                        after_minutes: 100 * 60,
                    }),
                    resume_note: None,
                },
            )
        });
        let done = human_task(&mut s, "completed Human task");
        ok(&mut s, |tx| {
            todo_attributes::set_state(
                tx,
                &Sender::Human,
                TodoSetStateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    object: done.todo.object,
                    channel_id: "crew-b/lobby".into(),
                    expected_revision: done.todo.revision,
                    state: ManualTodoState::Done,
                },
            )
        });
        let delegated = human_task(&mut s, "delegated Human task");
        ok(&mut s, |tx| {
            todo_attributes::assign(
                tx,
                &Sender::Human,
                TodoAssignParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    object: delegated.todo.object,
                    channel_id: "crew-b/lobby".into(),
                    expected_revision: delegated.todo.revision,
                    owner: Some(MemberRef::Session { id: "w-b1".into() }),
                },
            )
        });
        let long = "查看".repeat(220);
        let authored = human_task(&mut s, &long);
        let now = NOW + 73 * 60 * MIN;
        let original = asks::get(s.conn(), "crew-b", &blocking).unwrap();
        let before = s.conn().total_changes();
        let brief = get(s.conn(), "crew-b", now).unwrap();
        assert_eq!(s.conn().total_changes(), before);
        assert!(!brief.human_waits_truncated);
        assert_eq!(
            brief
                .human_waits
                .iter()
                .map(|w| w.ask_id.as_str())
                .collect::<Vec<_>>(),
            [
                blocking.as_str(),
                future_default.ask.as_str(),
                authored.ask.ask.as_str()
            ]
        );
        let row = &brief.human_waits[0];
        assert_eq!(row.kind, AdminHumanBriefWaitKind::Question);
        assert_eq!(row.tier, WaitTierView::StillWanted);
        assert_eq!(
            row.notified_tier,
            WaitTierView::Fresh,
            "reading does not advance notices"
        );
        assert_eq!(row.blocking_work_ids, [work]);
        assert_eq!(row.source_message_id, original.message_id);
        assert_eq!(row.source_channel_id.as_deref(), Some("crew-a/human"));
        assert_eq!(
            brief.human_waits[1].default_due_at_ms,
            Some(NOW + 100 * 60 * MIN)
        );
        let task = &brief.human_waits[2];
        assert_eq!(task.kind, AdminHumanBriefWaitKind::HumanTask);
        assert_eq!(task.tier, WaitTierView::Fresh);
        assert!(task.source_message_id.is_none());
        assert!(task.question_truncated);
        assert_eq!(
            task.question,
            long[..long.floor_char_boundary(QUESTION_BYTES)]
        );
        assert_eq!(
            asks::get(s.conn(), "crew-b", &authored.ask.ask)
                .unwrap()
                .question,
            long
        );
        assert_eq!(
            get(s.conn(), "crew-a", now).unwrap().human_waits[0].ask_id,
            sibling.ask
        );
        assert_eq!(
            asks::get(s.conn(), "crew-b", &due_default).unwrap().state,
            crewd_proto::AskStateView::Open
        );

        let due = ok_at(&mut s, now, asks::due_escalation_page).0;
        assert_eq!(due.len(), 1);
        for next in due {
            ok_at(&mut s, now, |tx| asks::escalate_wait(tx, next));
        }
        let after = get(s.conn(), "crew-b", now).unwrap();
        assert_eq!(
            after.human_waits[0].notified_tier,
            WaitTierView::StillWanted
        );
        assert_eq!(after.human_waits[0].source_message_id, original.message_id);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let mut s = Store::open(&path).unwrap();
        let before = s.conn().total_changes();
        assert_eq!(
            serde_json::to_value(get(s.conn(), "crew-b", now).unwrap()).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        assert_eq!(s.conn().total_changes(), before);
        ok_at(&mut s, now, |tx| {
            asks::answer(tx, "crew-b", &blocking, "confirmed")
        });
        assert!(
            get(s.conn(), "crew-b", now)
                .unwrap()
                .human_waits
                .iter()
                .all(|w| w.ask_id != blocking)
        );
    }

    #[test]
    fn bounded_human_brief_keeps_current_waits_and_durable_patrol_separate() {
        let mut s = store();
        for n in 0..(HUMAN_ITEMS + 2) {
            human_task(&mut s, &format!("task {n}"));
        }
        for _ in 0..30 {
            ok(&mut s, patrol_runtime::tick);
        }
        let before = s.conn().total_changes();
        let p = get(s.conn(), "crew-b", NOW).unwrap();
        assert_eq!(p.human_waits.len(), HUMAN_ITEMS);
        assert!(p.human_waits_truncated);
        assert_eq!(p.patrol.state, AdminHumanBriefPatrolState::Complete);
        assert_eq!(p.patrol.completed_at_ms, Some(NOW));
        assert_eq!(s.conn().total_changes(), before);
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        let p = get(s.conn(), "crew-b", NOW).unwrap();
        assert_eq!(p.patrol.state, AdminHumanBriefPatrolState::NotRecorded);
        assert_eq!(
            p.human_waits.len(),
            HUMAN_ITEMS,
            "restore fencing applies to observations, not current work"
        );
        crewd_store::meta_set(s.conn(), "patrol.run.v1:crew-b", "damaged").unwrap();
        let p = get(s.conn(), "crew-b", NOW).unwrap();
        assert_eq!(p.patrol.state, AdminHumanBriefPatrolState::Unavailable);
        assert_eq!(p.human_waits.len(), HUMAN_ITEMS);
    }

    #[test]
    fn human_brief_reports_unresolved_cases_without_equating_execution_with_outcome() {
        let mut s = store();
        let (case, reg) = crate::admin_case_execution::tests::running_case(&mut s);
        let read = get(s.conn(), "crew-a", NOW).unwrap();
        assert_eq!(read.case_coverage, AdminHumanBriefCaseCoverage::Complete);
        assert_eq!(read.cases.len(), 1);
        assert_eq!(read.cases[0].case_id, case.id);
        assert_eq!(
            read.cases[0].execution,
            crewd_proto::AdminCaseExecution::Running
        );
        assert!(read.cases[0].outcome_phase.is_none());
        assert!(get(s.conn(), "crew-b", NOW).unwrap().cases.is_empty());
        for target in case.targets {
            use crate::admin_case_execution as execution;
            ok(&mut s, |tx| {
                let permit = execution::validate_effect(
                    tx,
                    &reg.witness,
                    execution::ActionScope::WorkClose {
                        id: target.id.clone(),
                        kind: crewd_proto::WorkClosureKind::Done,
                    },
                )?;
                execution::tests::close(tx, &reg.session.id, &target.id)?;
                execution::finish_effect(tx, permit)
            });
        }
        assert_eq!(get(s.conn(), "crew-a", NOW).unwrap().cases.len(), 1);
        let outcome = ok(&mut s, |tx| {
            crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
        });
        assert_eq!(outcome.phase, AdminCaseOutcomePhase::Resolved);
        assert!(get(s.conn(), "crew-a", NOW).unwrap().cases.is_empty());
        crewd_store::meta_set(
            s.conn(),
            &admin_cases::key("crew-a", &case.id).unwrap(),
            "damaged",
        )
        .unwrap();
        assert_eq!(
            get(s.conn(), "crew-a", NOW).unwrap().case_coverage,
            AdminHumanBriefCaseCoverage::Unavailable
        );
    }
}
