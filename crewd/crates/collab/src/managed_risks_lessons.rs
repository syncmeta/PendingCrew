//! Scoped risk and lesson commands. These records do not schedule any work.
use super::{
    Actor, PAGE_LIMIT, about_refusal, decision_actor, decision_write_refusal, forbidden, invalid,
    missing, number, object_number,
};
use crate::{
    CollabError, Result, Sender, crews,
    refusal::{Code, Refusal},
    resource_acl, sessions,
};
use crewd_proto::{
    ResourceActor,
    managed_objects::{
        Change, Lesson, LessonApplicability, LessonDraft, LessonPage, LessonSource, LessonStatus,
        Object, Risk, RiskAbout, RiskDraft, RiskMitigation, RiskOccurrence, RiskOwner, RiskPage,
        RiskStatus,
    },
};
use crewd_store::{
    StoreError, managed_decisions, managed_objects, managed_risks_lessons as db,
    object_refs::{self, ObjectKind},
};
use rusqlite::Connection;
use std::collections::HashSet;

const REVIEW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const MAX_LINKS: usize = 8;

pub(super) fn risk(c: &Connection, id: &str, field: &str) -> Result<(i64, Risk)> {
    let number = object_number(id, 'R', field)?;
    Ok((number, db::risk(c, number)?.ok_or_else(|| missing(id))?))
}

fn lesson(c: &Connection, id: &str, field: &str) -> Result<(i64, Lesson)> {
    let number = object_number(id, 'L', field)?;
    Ok((number, db::lesson(c, number)?.ok_or_else(|| missing(id))?))
}

pub fn get(c: &Connection, id: &str) -> Result<Object> {
    if id.starts_with('R') {
        Ok(Object::Risk {
            risk: risk(c, id, "id")?.1,
        })
    } else {
        Ok(Object::Lesson {
            lesson: lesson(c, id, "id")?.1,
        })
    }
}

fn page<T>(
    c: &Connection,
    after: Option<&str>,
    prefix: char,
    numbers: impl FnOnce(&Connection, Option<i64>, u16) -> crewd_store::Result<Vec<i64>>,
    get: impl Fn(&Connection, i64) -> crewd_store::Result<Option<T>>,
) -> Result<(Vec<T>, Option<String>)> {
    let before = after
        .map(|id| object_number(id, prefix, "after"))
        .transpose()?;
    let ids = numbers(c, before, PAGE_LIMIT + 1)?;
    let next_after = (ids.len() > PAGE_LIMIT as usize)
        .then(|| format!("{prefix}{}", ids[PAGE_LIMIT as usize - 1]));
    let items = ids
        .into_iter()
        .take(PAGE_LIMIT as usize)
        .map(|id| get(c, id)?.ok_or_else(|| missing(&format!("{prefix}{id}"))))
        .collect::<Result<_>>()?;
    Ok((items, next_after))
}

pub fn risks(c: &Connection, actor: Actor<'_>, after: Option<&str>) -> Result<RiskPage> {
    actor.principal(c)?;
    let (items, next_after) = page(c, after, 'R', db::risk_numbers, db::risk)?;
    Ok(RiskPage { items, next_after })
}

pub fn lessons(c: &Connection, actor: Actor<'_>, after: Option<&str>) -> Result<LessonPage> {
    actor.principal(c)?;
    let (items, next_after) = page(c, after, 'L', db::lesson_numbers, db::lesson)?;
    Ok(LessonPage { items, next_after })
}

fn sender(actor: Actor<'_>) -> Sender {
    match actor {
        Actor::Human(_) => Sender::Human,
        Actor::Session(id) => Sender::Session(id.into()),
    }
}

fn is_author(actor: Actor<'_>, author: &ResourceActor) -> bool {
    matches!(actor, Actor::Session(id) if author.kind == "session" && author.session_id.as_deref() == Some(id))
}

fn is_owner(actor: Actor<'_>, owner: &RiskOwner) -> bool {
    match (actor, owner) {
        (Actor::Human(_), RiskOwner::Human {}) => true,
        (Actor::Session(id), RiskOwner::Session { session_id }) => id == session_id,
        _ => false,
    }
}

fn default_owner(actor: Actor<'_>) -> RiskOwner {
    match actor {
        Actor::Human(_) => RiskOwner::Human {},
        Actor::Session(id) => RiskOwner::Session {
            session_id: id.into(),
        },
    }
}

fn check_owner(c: &Connection, crew: &str, owner: &RiskOwner) -> Result<()> {
    if let RiskOwner::Session { session_id } = owner {
        check_ref_id(session_id, "owner")?;
        let candidate = sessions::get(c, session_id).map_err(|e| about_refusal(e, session_id))?;
        if candidate.crew_id != crew || candidate.role == crewd_proto::Role::Admin {
            return Err(forbidden());
        }
    }
    Ok(())
}

fn check_ref_id(id: &str, field: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 || id.trim() != id || id.chars().any(char::is_control) {
        return Err(invalid(field));
    }
    Ok(())
}

fn check_links<T: Eq + std::hash::Hash>(
    links: &[T],
    field: &str,
    verify: impl Fn(&T) -> Result<()>,
) -> Result<()> {
    if links.iter().collect::<HashSet<_>>().len() != links.len() {
        return Err(invalid(field));
    }
    for link in links {
        verify(link)?;
    }
    Ok(())
}

fn check_crew(c: &Connection, id: &str, field: &str) -> Result<()> {
    check_ref_id(id, field)?;
    crews::get(c, id).map_err(|e| about_refusal(e, id))?;
    Ok(())
}

fn check_work(c: &Connection, id: &str, field: &str) -> Result<()> {
    object_number(id, 'W', field)?;
    crate::work::get(c, id).map_err(|e| about_refusal(e, id))?;
    Ok(())
}

fn check_risk_draft(c: &Connection, draft: &RiskDraft) -> Result<()> {
    use managed_decisions::valid_text;
    if !valid_text(&draft.summary, 120)
        || !draft
            .mitigation
            .as_deref()
            .is_none_or(|text| valid_text(text, 1500))
        || !(1..=MAX_LINKS).contains(&draft.about.len())
        || draft.mitigated_by.len() > MAX_LINKS
    {
        return Err(invalid("draft"));
    }
    check_links(&draft.about, "about", |link| match link {
        RiskAbout::Crew { id } => check_crew(c, id, "about"),
        RiskAbout::Work { id } => check_work(c, id, "about"),
        RiskAbout::Requirement { id } => {
            object_number(id, '#', "about")?;
            crate::requirements::get(c, id).map_err(|e| about_refusal(e, id))?;
            Ok(())
        }
    })?;
    check_links(&draft.mitigated_by, "mitigatedBy", |link| match link {
        RiskMitigation::Work { id } => check_work(c, id, "mitigatedBy"),
        RiskMitigation::Decision { id } => {
            managed_decisions::get(c, object_number(id, 'D', "mitigatedBy")?)?
                .ok_or_else(|| missing(id))?;
            Ok(())
        }
    })
}

fn check_ask(c: &Connection, actor: Actor<'_>, id: &str) -> Result<()> {
    let number = object_number(id, 'H', "learnedFrom")?;
    let reference =
        object_refs::get(c, ObjectKind::HumanTask, number)?.ok_or_else(|| missing(id))?;
    let by = sender(actor);
    resource_acl::actor(c, &by, &reference.crew_id, false).map_err(|e| about_refusal(e, id))?;
    let ask = crate::asks::get(c, &reference.crew_id, id).map_err(|e| about_refusal(e, id))?;
    if let (Actor::Session(session), Some(message_id)) = (actor, ask.message_id) {
        let announcement =
            crate::messages::get(c, &message_id).map_err(|e| about_refusal(e, id))?;
        if !crate::channels::readable_by(c, &announcement.channel_id, session)? {
            return Err(forbidden());
        }
    }
    Ok(())
}

fn check_lesson_draft(
    c: &Connection,
    actor: Actor<'_>,
    draft: &LessonDraft,
    occurrence: bool,
) -> Result<()> {
    use managed_decisions::valid_text;
    let source_count = draft.learned_from.len();
    if !valid_text(&draft.happened, 1500)
        || !valid_text(&draft.lesson, 120)
        || !draft
            .detection
            .as_deref()
            .is_none_or(|text| valid_text(text, 1500))
        || source_count > MAX_LINKS
        || (occurrence && source_count == MAX_LINKS)
        || draft.evidence.len() > MAX_LINKS
        || draft.applies_to.len() > MAX_LINKS
        || draft.evidence.iter().any(|item| !valid_text(item, 1500))
        || draft.evidence.iter().collect::<HashSet<_>>().len() != draft.evidence.len()
    {
        return Err(invalid("draft"));
    }
    check_links(&draft.learned_from, "learnedFrom", |link| match link {
        LessonSource::Work { id } => check_work(c, id, "learnedFrom"),
        LessonSource::Ask { id } => check_ask(c, actor, id),
        // The occurrence command derives the sole authoritative risk source.
        LessonSource::Risk { .. } => Err(invalid("learnedFrom")),
    })?;
    check_links(&draft.applies_to, "appliesTo", |link| match link {
        LessonApplicability::Crew { id } => check_crew(c, id, "appliesTo"),
        LessonApplicability::Work { id } => check_work(c, id, "appliesTo"),
        LessonApplicability::Table { id } => {
            managed_objects::table(c, object_number(id, 'T', "appliesTo")?)?
                .ok_or_else(|| missing(id))?;
            Ok(())
        }
    })
}

pub(super) enum Authorized {
    New,
    Risk(i64, Risk),
    Lesson(i64, Lesson),
}

pub(super) fn authorize(
    c: &Connection,
    actor: Actor<'_>,
    crew: &str,
    change: &Change,
) -> Result<Option<(String, Authorized)>> {
    let target = match change {
        Change::RecordRisk { .. } | Change::RecordLesson { .. } => {
            return Ok(Some((
                actor.owner(c, crew, true).map_err(decision_write_refusal)?,
                Authorized::New,
            )));
        }
        Change::UpdateRisk { risk_id, .. }
        | Change::AssignRisk { risk_id, .. }
        | Change::ReviewRisk { risk_id, .. }
        | Change::SetRiskStatus { risk_id, .. }
        | Change::RecordRiskOccurrence { risk_id, .. } => {
            let (id, risk) = risk(c, risk_id, "riskId")?;
            if risk.crew_id != crew {
                return Err(forbidden());
            }
            let owner = is_owner(actor, &risk.owner);
            let allowed = if matches!(change, Change::ReviewRisk { .. }) {
                owner
            } else {
                match actor {
                    Actor::Human(_) => true,
                    Actor::Session(session) => {
                        ((owner || is_author(actor, &risk.author))
                            && sessions::get(c, session)?.crew_id == crew)
                            || crews::acts_as_captain_of(c, session, crew)?
                    }
                }
            };
            if !allowed {
                return Err(forbidden());
            }
            resource_acl::actor(c, &sender(actor), crew, true).map_err(decision_write_refusal)?;
            Authorized::Risk(id, risk)
        }
        Change::ConfirmLesson { lesson_id, .. } | Change::StaleLesson { lesson_id, .. } => {
            let (id, lesson) = lesson(c, lesson_id, "lessonId")?;
            if lesson.crew_id != crew {
                return Err(forbidden());
            }
            if matches!(change, Change::ConfirmLesson { .. }) {
                crews::refuse_archived(c, crew).map_err(decision_write_refusal)?;
                if let Actor::Session(session) = actor {
                    let member = sessions::get(c, session)?;
                    crews::refuse_archived(c, &member.crew_id).map_err(decision_write_refusal)?;
                }
            } else {
                if let Actor::Session(session) = actor {
                    let own_author = is_author(actor, &lesson.author)
                        && sessions::get(c, session)?.crew_id == crew;
                    if !own_author && !crews::acts_as_captain_of(c, session, crew)? {
                        return Err(forbidden());
                    }
                }
                resource_acl::actor(c, &sender(actor), crew, true)
                    .map_err(decision_write_refusal)?;
            }
            Authorized::Lesson(id, lesson)
        }
        _ => return Ok(None),
    };
    Ok(Some((actor.principal(c)?, target)))
}

fn risk_changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedRiskChanged))
}

fn lesson_changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedLessonChanged))
}

fn require_risk_revision(expected: &str, risk: &Risk) -> Result<i64> {
    let revision = number(expected, "expectedRevision")?;
    if risk.revision != expected {
        return Err(risk_changed());
    }
    Ok(revision)
}

fn require_open_risk(expected: &str, risk: &Risk) -> Result<i64> {
    let revision = require_risk_revision(expected, risk)?;
    if !matches!(risk.status, RiskStatus::Open | RiskStatus::Mitigated) {
        return Err(risk_changed());
    }
    Ok(revision)
}

fn require_active_lesson(expected: &str, lesson: &Lesson) -> Result<i64> {
    let revision = number(expected, "expectedRevision")?;
    if lesson.revision != expected || lesson.status != LessonStatus::Active {
        return Err(lesson_changed());
    }
    Ok(revision)
}

fn next_review(now_ms: i64) -> Result<String> {
    let due = now_ms
        .checked_add(REVIEW_MS)
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .ok_or_else(|| StoreError::Invalid("managed risk review date out of range".into()))?;
    Ok(due.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn save_risk(c: &Connection, id: i64, revision: i64, mut risk: Risk, now: &str) -> Result<Object> {
    risk.updated_at = now.into();
    Ok(Object::Risk {
        risk: db::save_risk(c, id, revision, risk)?,
    })
}

pub(super) fn apply(
    c: &Connection,
    actor: Actor<'_>,
    crew: &str,
    target: Authorized,
    change: Change,
    now_ms: i64,
    now: &str,
) -> Result<(Object, Option<RiskOccurrence>)> {
    let object = match (target, change) {
        (Authorized::New, Change::RecordRisk { draft, owner }) => {
            check_risk_draft(c, &draft)?;
            let owner = owner.unwrap_or_else(|| default_owner(actor));
            check_owner(c, crew, &owner)?;
            Object::Risk {
                risk: db::record_risk(
                    c,
                    crew,
                    draft,
                    owner,
                    decision_actor(actor),
                    now,
                    next_review(now_ms)?,
                )?,
            }
        }
        (
            Authorized::Risk(id, mut risk),
            Change::UpdateRisk {
                expected_revision,
                draft,
                ..
            },
        ) => {
            let revision = require_open_risk(&expected_revision, &risk)?;
            check_risk_draft(c, &draft)?;
            risk.draft = draft;
            if is_owner(actor, &risk.owner) {
                risk.next_review_at = next_review(now_ms)?;
            }
            save_risk(c, id, revision, risk, now)?
        }
        (
            Authorized::Risk(id, mut risk),
            Change::AssignRisk {
                expected_revision,
                owner,
                ..
            },
        ) => {
            let revision = require_open_risk(&expected_revision, &risk)?;
            check_owner(c, crew, &owner)?;
            if risk.owner == owner {
                return Err(invalid("owner"));
            }
            risk.owner = owner;
            save_risk(c, id, revision, risk, now)?
        }
        (
            Authorized::Risk(id, mut risk),
            Change::ReviewRisk {
                expected_revision, ..
            },
        ) => {
            let revision = require_open_risk(&expected_revision, &risk)?;
            risk.next_review_at = next_review(now_ms)?;
            save_risk(c, id, revision, risk, now)?
        }
        (
            Authorized::Risk(id, mut risk),
            Change::SetRiskStatus {
                expected_revision,
                status,
                reason,
                ..
            },
        ) => {
            let revision = require_risk_revision(&expected_revision, &risk)?;
            let allowed = matches!(
                (&risk.status, &status),
                (RiskStatus::Open, RiskStatus::Mitigated | RiskStatus::Closed)
                    | (RiskStatus::Mitigated, RiskStatus::Open | RiskStatus::Closed)
                    | (RiskStatus::Occurred, RiskStatus::Closed)
            );
            if !allowed {
                return Err(risk_changed());
            }
            if !managed_decisions::valid_text(&reason, 1500) {
                return Err(invalid("reason"));
            }
            if matches!(status, RiskStatus::Open | RiskStatus::Mitigated)
                && is_owner(actor, &risk.owner)
            {
                risk.next_review_at = next_review(now_ms)?;
            }
            risk.status = status;
            risk.status_reason = Some(reason);
            save_risk(c, id, revision, risk, now)?
        }
        (
            Authorized::Risk(id, mut risk),
            Change::RecordRiskOccurrence {
                expected_revision,
                mut lesson_draft,
                ..
            },
        ) => {
            let revision = require_open_risk(&expected_revision, &risk)?;
            check_lesson_draft(c, actor, &lesson_draft, true)?;
            lesson_draft.learned_from.push(LessonSource::Risk {
                id: risk.id.clone(),
            });
            let lesson =
                db::record_lesson(c, crew, lesson_draft, decision_actor(actor), now, Some(id))?;
            risk.status = RiskStatus::Occurred;
            risk.status_reason = None;
            risk.updated_at = now.into();
            let updated = db::save_risk(c, id, revision, risk)?;
            let occurrence = RiskOccurrence {
                risk_id: updated.id,
                original_risk_revision: expected_revision,
                result_risk_revision: updated.revision,
                lesson_id: lesson.id.clone(),
                lesson_revision: lesson.revision.clone(),
            };
            return Ok((Object::Lesson { lesson }, Some(occurrence)));
        }
        (Authorized::New, Change::RecordLesson { draft }) => {
            check_lesson_draft(c, actor, &draft, false)?;
            Object::Lesson {
                lesson: db::record_lesson(c, crew, draft, decision_actor(actor), now, None)?,
            }
        }
        (
            Authorized::Lesson(id, mut lesson),
            Change::ConfirmLesson {
                expected_revision, ..
            },
        ) => {
            let revision = require_active_lesson(&expected_revision, &lesson)?;
            lesson.last_checked_at = now.into();
            Object::Lesson {
                lesson: db::save_lesson(c, id, revision, lesson)?,
            }
        }
        (
            Authorized::Lesson(id, mut lesson),
            Change::StaleLesson {
                expected_revision,
                reason,
                ..
            },
        ) => {
            let revision = require_active_lesson(&expected_revision, &lesson)?;
            if !managed_decisions::valid_text(&reason, 1500) {
                return Err(invalid("reason"));
            }
            lesson.status = LessonStatus::Stale;
            lesson.stale_reason = Some(reason);
            Object::Lesson {
                lesson: db::save_lesson(c, id, revision, lesson)?,
            }
        }
        _ => return Err(invalid("change")),
    };
    Ok((object, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{managed_objects as managed, testkit::NOW};
    use crewd_proto::managed_objects::{
        DecisionAbout, DecisionDraft, Lookup, Receipt, RiskLevel, Submit,
    };
    use crewd_store::Store;

    const HUMAN: Actor<'static> = Actor::Human("local-anchor");
    const OWNER: Actor<'static> = Actor::Session("w-a1");
    const CAPTAIN: Actor<'static> = Actor::Session("cap-a");
    const OTHER: Actor<'static> = Actor::Session("w-b1");

    fn fixture() -> Store {
        let store = crate::testkit::store();
        crewd_store::meta_set(store.conn(), "dbId", "risk-lesson-fixture").unwrap();
        store
    }
    fn request(s: &Store, crew: &str, change: Change) -> Submit {
        Submit {
            scope: managed::scope(s.conn()).unwrap(),
            crew_id: crew.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change,
        }
    }
    fn call(s: &mut Store, actor: Actor<'_>, req: Submit, now_ms: i64) -> Result<Receipt> {
        let (value, changes) = crate::run(s, now_ms, |tx| managed::submit(tx, actor, req))?;
        assert!(
            changes.iter().all(|change| matches!(
                change,
                crate::Change::OntologyChanged { .. } | crate::Change::Progress { .. }
            )),
            "risk and lesson commands must only publish invalidation and progress"
        );
        Ok(value)
    }
    fn risk_draft() -> RiskDraft {
        RiskDraft {
            summary: "旧服务仍然写入已迁移的数据".into(),
            impact: RiskLevel::High,
            likelihood: Some(RiskLevel::Medium),
            mitigation: Some("切换前停止旧写入".into()),
            about: vec![RiskAbout::Crew {
                id: "crew-a".into(),
            }],
            mitigated_by: vec![],
        }
    }
    fn lesson_draft() -> LessonDraft {
        LessonDraft {
            happened: "切换时旧服务继续接收数据。两端出现不同版本。".into(),
            lesson: "切换前停止旧写入".into(),
            detection: Some("查看两端写入代号".into()),
            learned_from: vec![],
            evidence: vec!["fixture:cutover".into()],
            applies_to: vec![LessonApplicability::Crew {
                id: "crew-a".into(),
            }],
        }
    }
    fn issue_risk(s: &mut Store, crew: &str, actor: Actor<'_>) -> (Submit, Risk) {
        let req = request(
            s,
            crew,
            Change::RecordRisk {
                draft: risk_draft(),
                owner: None,
            },
        );
        let Object::Risk { risk } = call(s, actor, req.clone(), NOW).unwrap().object else {
            panic!()
        };
        (req, risk)
    }
    fn fetch_risk(s: &Store, id: &str) -> Risk {
        let Object::Risk { risk } = managed::get(s.conn(), OTHER, id).unwrap() else {
            panic!()
        };
        risk
    }
    fn fetch_lesson(s: &Store, id: &str) -> Lesson {
        let Object::Lesson { lesson } = managed::get(s.conn(), OTHER, id).unwrap() else {
            panic!()
        };
        lesson
    }

    #[test]
    fn owner_review_deadline_and_passive_exact_due_selection() {
        let mut s = fixture();
        let (_, original) = issue_risk(&mut s, "crew-a", OWNER);
        assert_eq!(
            original.owner,
            RiskOwner::Session {
                session_id: "w-a1".into()
            }
        );
        assert_eq!(
            original.next_review_at,
            crewd_store::ts_from_ms(NOW + REVIEW_MS)
        );
        assert!(
            db::due_owned_risks(s.conn(), "w-a1", NOW + REVIEW_MS - 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db::due_owned_risks(s.conn(), "w-a1", NOW + REVIEW_MS).unwrap(),
            [original.clone()]
        );
        assert!(
            db::due_owned_risks(s.conn(), "w-a2", NOW + REVIEW_MS)
                .unwrap()
                .is_empty()
        );

        let id = original.id.clone();
        let check = request(
            &s,
            "crew-a",
            Change::ReviewRisk {
                risk_id: id.clone(),
                expected_revision: "1".into(),
            },
        );
        assert!(matches!(
            call(&mut s, CAPTAIN, check.clone(), NOW + 1),
            Err(CollabError::Forbidden(_))
        ));
        call(&mut s, OWNER, check, NOW + 2).unwrap();
        let change = request(
            &s,
            "crew-a",
            Change::UpdateRisk {
                risk_id: id.clone(),
                expected_revision: "2".into(),
                draft: risk_draft(),
            },
        );
        call(&mut s, CAPTAIN, change, NOW + 3).unwrap();
        assert_eq!(
            fetch_risk(&s, &id).next_review_at,
            crewd_store::ts_from_ms(NOW + 2 + REVIEW_MS)
        );

        let transfer = request(
            &s,
            "crew-a",
            Change::AssignRisk {
                risk_id: id.clone(),
                expected_revision: "3".into(),
                owner: RiskOwner::Session {
                    session_id: "w-a2".into(),
                },
            },
        );
        call(&mut s, OWNER, transfer, NOW + 4).unwrap();
        assert!(
            db::due_owned_risks(s.conn(), "w-a1", NOW + 4 + REVIEW_MS)
                .unwrap()
                .is_empty()
        );
        let moved = fetch_risk(&s, &id);
        assert_eq!(
            moved.next_review_at,
            crewd_store::ts_from_ms(NOW + 2 + REVIEW_MS)
        );
        assert_eq!(
            db::due_owned_risks(s.conn(), "w-a2", NOW + 4 + REVIEW_MS).unwrap(),
            [moved]
        );

        let mitigate = request(
            &s,
            "crew-a",
            Change::SetRiskStatus {
                risk_id: id.clone(),
                expected_revision: "4".into(),
                status: RiskStatus::Mitigated,
                reason: "停止了旧服务".into(),
            },
        );
        call(&mut s, Actor::Session("w-a2"), mitigate, NOW + 5).unwrap();
        assert_eq!(
            fetch_risk(&s, &id).next_review_at,
            crewd_store::ts_from_ms(NOW + 5 + REVIEW_MS)
        );
        assert!(
            db::due_owned_risks(s.conn(), "w-a2", NOW + 4 + REVIEW_MS)
                .unwrap()
                .is_empty()
        );
        crate::testkit::ok(&mut s, |tx| {
            crews::archive(tx, "crew-b")?;
            crews::archive(tx, "crew-a")
        });
        assert!(
            db::due_owned_risks(s.conn(), "w-a2", NOW + 5 + REVIEW_MS)
                .unwrap()
                .is_empty()
        );
        assert!(fetch_risk(&s, &id).frozen);
    }

    #[test]
    fn occurrence_pairs_both_revisions_rolls_back_faults_and_retains_link_after_close() {
        let mut s = fixture();
        let (_, original) = issue_risk(&mut s, "crew-a", OWNER);
        let mut proposed = lesson_draft();
        proposed.learned_from.push(LessonSource::Risk {
            id: original.id.clone(),
        });
        let bypass = request(&s, "crew-a", Change::RecordLesson { draft: proposed });
        assert!(matches!(
            call(&mut s, OWNER, bypass, NOW),
            Err(CollabError::Invalid(_))
        ));
        let command = request(
            &s,
            "crew-a",
            Change::RecordRiskOccurrence {
                risk_id: original.id.clone(),
                expected_revision: "1".into(),
                lesson_draft: lesson_draft(),
            },
        );
        assert!(matches!(
            call(&mut s, OTHER, command.clone(), NOW),
            Err(CollabError::Forbidden(_))
        ));
        for statement in [
            "BEFORE UPDATE ON managed_risks",
            "BEFORE INSERT ON managed_object_receipts",
        ] {
            s.conn()
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_occurrence {statement}
              BEGIN SELECT RAISE(ABORT,'fixture atomic occurrence'); END;"
                ))
                .unwrap();
            assert!(call(&mut s, OWNER, command.clone(), NOW + 1).is_err());
            assert_eq!(fetch_risk(&s, &original.id), original);
            assert_eq!(
                crate::testkit::count(&s, "SELECT count(*) FROM managed_lessons"),
                0
            );
            assert_eq!(
                crate::testkit::count(&s, "SELECT count(*) FROM object_refs WHERE kind='lesson'"),
                0
            );
            assert_eq!(
                managed::lookup(
                    s.conn(),
                    OWNER,
                    &command.scope,
                    "crew-a",
                    &command.request_id
                )
                .unwrap(),
                Lookup::NotFound
            );
            s.conn()
                .execute_batch("DROP TRIGGER fail_occurrence")
                .unwrap();
        }
        let result = call(&mut s, OWNER, command.clone(), NOW + 2).unwrap();
        let effect = result.occurrence.as_ref().unwrap();
        assert_eq!(effect.risk_id, original.id);
        assert_eq!(effect.original_risk_revision, "1");
        assert_eq!(effect.result_risk_revision, "2");
        let l = fetch_lesson(&s, &effect.lesson_id);
        assert_eq!(effect.lesson_revision, l.revision);
        assert_eq!(l.risk_id.as_deref(), Some(original.id.as_str()));
        assert_eq!(
            l.draft.learned_from,
            [LessonSource::Risk {
                id: original.id.clone()
            }]
        );
        let paired = fetch_risk(&s, &original.id);
        assert_eq!(paired.became_lesson_id.as_deref(), Some(l.id.as_str()));
        assert_eq!(paired.status, RiskStatus::Occurred);
        assert_eq!(paired.next_review_at, original.next_review_at);

        let mut competing = command.clone();
        competing.request_id = uuid::Uuid::new_v4().to_string();
        assert!(matches!(call(&mut s, OWNER, competing, NOW + 3),
            Err(CollabError::Conflict(r)) if r.code == Code::ManagedRiskChanged));
        let mut tampered = command.clone();
        if let Change::RecordRiskOccurrence { lesson_draft, .. } = &mut tampered.change {
            lesson_draft.lesson.push_str(" [other]");
        }
        assert!(matches!(call(&mut s, OWNER, tampered, NOW + 3),
            Err(CollabError::Conflict(r)) if r.code == Code::ResourceRequestConflict));

        let close = request(
            &s,
            "crew-a",
            Change::SetRiskStatus {
                risk_id: original.id.clone(),
                expected_revision: "2".into(),
                status: RiskStatus::Closed,
                reason: "记录已完成".into(),
            },
        );
        call(&mut s, OWNER, close, NOW + 4).unwrap();
        let closed = fetch_risk(&s, &original.id);
        assert_eq!(closed.became_lesson_id.as_deref(), Some(l.id.as_str()));
        assert_eq!(closed.status, RiskStatus::Closed);
        assert_eq!(
            managed::lookup(
                s.conn(),
                OWNER,
                &command.scope,
                "crew-a",
                &command.request_id
            )
            .unwrap(),
            Lookup::Known {
                receipt: result.clone()
            }
        );
        assert_eq!(call(&mut s, OWNER, command, NOW + 4).unwrap(), result);
    }

    #[test]
    fn foreign_confirmation_only_recovers_own_exact_receipt_and_stale_stays_stale() {
        let mut s = fixture();
        let record = request(
            &s,
            "crew-a",
            Change::RecordLesson {
                draft: lesson_draft(),
            },
        );
        let Object::Lesson { lesson } = call(&mut s, OWNER, record.clone(), NOW).unwrap().object
        else {
            panic!()
        };
        let checked = request(
            &s,
            "crew-a",
            Change::ConfirmLesson {
                lesson_id: lesson.id.clone(),
                expected_revision: "1".into(),
            },
        );
        let original = call(&mut s, OTHER, checked.clone(), NOW + 10).unwrap();
        assert_eq!(
            fetch_lesson(&s, &lesson.id).last_checked_at,
            original.recorded_at
        );
        assert_eq!(
            managed::lookup(
                s.conn(),
                OTHER,
                &checked.scope,
                "crew-a",
                &checked.request_id
            )
            .unwrap(),
            Lookup::Known { receipt: original }
        );
        assert!(matches!(
            managed::lookup(s.conn(), OTHER, &record.scope, "crew-a", &record.request_id),
            Err(CollabError::Forbidden(_))
        ));
        assert_eq!(
            managed::lookup(
                s.conn(),
                HUMAN,
                &checked.scope,
                "crew-a",
                &checked.request_id
            )
            .unwrap(),
            Lookup::NotFound
        );

        let stale = request(
            &s,
            "crew-a",
            Change::StaleLesson {
                lesson_id: lesson.id.clone(),
                expected_revision: "2".into(),
                reason: "写入机制已更换".into(),
            },
        );
        assert!(matches!(
            call(&mut s, OTHER, stale.clone(), NOW + 20),
            Err(CollabError::Forbidden(_))
        ));
        call(&mut s, CAPTAIN, stale, NOW + 20).unwrap();
        let current = fetch_lesson(&s, &lesson.id);
        assert_eq!(current.status, LessonStatus::Stale);
        assert_eq!(current.last_checked_at, crewd_store::ts_from_ms(NOW + 10));
        let confirm = request(
            &s,
            "crew-a",
            Change::ConfirmLesson {
                lesson_id: lesson.id,
                expected_revision: "3".into(),
            },
        );
        assert!(matches!(call(&mut s, OTHER, confirm, NOW + 30),
            Err(CollabError::Conflict(r)) if r.code == Code::ManagedLessonChanged));
    }

    #[test]
    fn direct_decision_risk_reference_requires_authoritative_r_and_h_uses_source_access() {
        let mut s = fixture();
        let (_, risk) = issue_risk(&mut s, "crew-a", OWNER);
        let decision = |id| Change::RecordDecision {
            draft: DecisionDraft {
                question: "能否切换".into(),
                choice: "先阻止旧写入".into(),
                rationale: "源数据不能分叉".into(),
                alternatives: None,
                reversibility: None,
                about: vec![DecisionAbout::Risk { id }],
            },
        };
        let missing = request(&s, "crew-a", decision("R999999".into()));
        assert!(
            matches!(call(&mut s, OWNER, missing, NOW), Err(CollabError::NotFound(r)) if r.code == Code::ResourceNotFound)
        );
        let valid = request(&s, "crew-b", decision(risk.id));
        assert!(matches!(
            call(&mut s, OTHER, valid, NOW).unwrap().object,
            Object::Decision { .. }
        ));

        let ask = crate::testkit::ok(&mut s, |tx| {
            crate::asks::open(
                tx,
                "cap-x",
                crate::asks::OpenAsk {
                    question: "是否中止旧写入？".into(),
                    default: None,
                    resume_note: None,
                },
            )
        });
        let mut draft = lesson_draft();
        draft.learned_from.push(LessonSource::Ask { id: ask.ask });
        let private = request(&s, "crew-a", Change::RecordLesson { draft });
        assert!(matches!(
            call(&mut s, OWNER, private.clone(), NOW),
            Err(CollabError::Forbidden(_))
        ));
        let retained = call(&mut s, HUMAN, private, NOW).unwrap();
        let text = serde_json::to_string(&retained).unwrap();
        assert!(!text.contains("是否中止旧写入"));
        assert!(!text.contains(ask.message_id.as_deref().unwrap()));
    }

    #[test]
    fn guarded_v24_upgrade_keeps_dto_and_original_receipt_then_purge_both_new_objects() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open_data_dir(dir.path()).unwrap();
        s.conn().execute_batch(
            "INSERT INTO crews(id,title,created_at,updated_at) VALUES ('crew-a','A','t','t'),('crew-x','X','t','t');",
        ).unwrap();
        let table = request(
            &s,
            "crew-a",
            Change::CreateTable {
                title: "Before migration".into(),
                fields: vec![crewd_proto::managed_objects::Field {
                    name: "Value".into(),
                    field_type: crewd_proto::managed_objects::FieldType::Text,
                    nullable: false,
                }],
            },
        );
        let old = call(&mut s, HUMAN, table.clone(), NOW).unwrap();
        assert!(!serde_json::to_string(&old).unwrap().contains("occurrence"));
        s.conn()
            .execute_batch(
                "DROP TABLE public_command_purge_tombstones;
             DROP TABLE decision_ask_open_receipts;
             DROP TABLE decision_asks;
             DROP TABLE managed_lessons;
             DROP TABLE managed_risks;
             ALTER TABLE managed_object_receipts DROP COLUMN member_confirmation;
             PRAGMA user_version=24;",
            )
            .unwrap();
        drop(s);
        let mut s = Store::open_data_dir(dir.path()).unwrap();
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            26
        );
        assert_eq!(
            managed::lookup(s.conn(), HUMAN, &table.scope, "crew-a", &table.request_id).unwrap(),
            Lookup::Known { receipt: old }
        );

        let (_, risk) = issue_risk(&mut s, "crew-x", HUMAN);
        let occurrence = request(
            &s,
            "crew-x",
            Change::RecordRiskOccurrence {
                risk_id: risk.id.clone(),
                expected_revision: "1".into(),
                lesson_draft: lesson_draft(),
            },
        );
        let result = call(&mut s, HUMAN, occurrence.clone(), NOW).unwrap();
        let lesson_id = result.occurrence.unwrap().lesson_id;
        crate::testkit::ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        let Object::Risk { risk } = managed::get(s.conn(), HUMAN, &risk.id).unwrap() else {
            panic!()
        };
        let Object::Lesson { lesson } = managed::get(s.conn(), HUMAN, &lesson_id).unwrap() else {
            panic!()
        };
        assert!(risk.frozen && lesson.frozen);
        let denied = request(
            &s,
            "crew-x",
            Change::ConfirmLesson {
                lesson_id: lesson_id.clone(),
                expected_revision: "1".into(),
            },
        );
        assert!(
            matches!(call(&mut s, HUMAN, denied, NOW), Err(CollabError::Conflict(r)) if r.code == Code::ResourceFrozen)
        );
        crate::testkit::ok(&mut s, |tx| crews::purge(tx, "crew-x"));
        assert!(managed::get(s.conn(), HUMAN, &risk.id).is_err());
        assert!(managed::get(s.conn(), HUMAN, &lesson_id).is_err());
        assert_eq!(
            crate::testkit::count(
                &s,
                "SELECT count(*) FROM managed_object_receipts WHERE crew_id='crew-x'"
            ),
            0
        );
        let (_, next) = issue_risk(&mut s, "crew-a", HUMAN);
        assert!(
            object_number(&next.id, 'R', "id").unwrap()
                > object_number(&lesson_id, 'L', "id").unwrap()
        );
    }
}
