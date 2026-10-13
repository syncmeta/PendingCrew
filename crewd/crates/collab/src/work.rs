//! Bounded global views over the authoritative work state.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, asks, crews};
use crewd_proto::{WorkEvidence, WorkListParams, WorkPage, WorkPlan};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_work::{Blocker, DoneLevel, Evidence, Status, WorkBook, WorkItem};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
fn invalid(detail: &str) -> CollabError {
    StoreError::Invalid(detail.into()).into()
}
fn number(id: &str) -> Result<i64> {
    match id.strip_prefix('W').and_then(|n| n.parse::<i64>().ok()) {
        Some(n) if n > 0 && id == format!("W{n}") => Ok(n),
        _ => Err(CollabError::Invalid(
            Refusal::new(Code::WorkBadId).arg("input", id),
        )),
    }
}
fn reference(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    local: impl std::fmt::Display,
) -> Result<String> {
    object_refs::resolve(conn, kind, crew, &format!("work:{local}"))?
        .map(|r| r.id())
        .ok_or_else(|| invalid("work link has no issued global identity"))
}
fn project(conn: &Connection, r: &ObjectReference, book: &WorkBook) -> Result<WorkPlan> {
    let local = r
        .source_id
        .strip_prefix("work:")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| invalid("plan reference source identity is invalid"))?;
    let item: &WorkItem = book
        .state()
        .items
        .get(&local)
        .ok_or_else(|| invalid("plan reference has no authoritative work item"))?;
    let status = match item.status {
        Status::Open => "open",
        Status::Active => "active",
        Status::Blocked => "blocked",
        Status::Done => "done",
        Status::Dropped => "dropped",
    };
    let level = match item.body.level {
        DoneLevel::Local => "local",
        DoneLevel::Public => "public",
        DoneLevel::Production => "production",
    };
    let plans = |id| reference(conn, ObjectKind::Plan, &r.crew_id, id);
    let evidence = item.evidence.as_ref().map(|e| {
        let (kind, value) = match e {
            Evidence::Commit(v) => ("commit", v),
            Evidence::Url(v) => ("url", v),
            Evidence::Text(v) => ("text", v),
        };
        WorkEvidence {
            kind: kind.into(),
            value: value.clone(),
        }
    });
    Ok(WorkPlan {
        id: r.id(),
        crew_id: r.crew_id.clone(),
        title: item.title.clone(),
        goal: item.body.goal.clone(),
        level: level.into(),
        acceptance: item.body.acceptance.clone(),
        owner: item.owner.as_ref().map(ToString::to_string),
        status: status.into(),
        parent: item.parent.map(plans).transpose()?,
        depends_on: item
            .depends_on
            .iter()
            .copied()
            .map(plans)
            .collect::<Result<_>>()?,
        sources: item
            .sources
            .iter()
            .map(|id| reference(conn, ObjectKind::Requirement, &r.crew_id, id))
            .collect::<Result<_>>()?,
        blocked_on: item
            .blocked_on
            .iter()
            .map(|b| match b {
                Blocker::Human(id) => reference(conn, ObjectKind::HumanTask, &r.crew_id, id),
                Blocker::Item(id) => plans(*id),
            })
            .collect::<Result<_>>()?,
        next_check_at: item.next_check_at.map(|t| crewd_store::ts_from_ms(t.0)),
        revision: item.revision.to_string(),
        evidence,
        note: item.last_report.as_ref().map(|n| n.text.clone()),
        created_at: crewd_store::ts_from_ms(item.created_at.0),
        updated_at: crewd_store::ts_from_ms(item.updated_at.0),
    })
}
/// All organization objects remain readable, including removed crews.
pub fn get(conn: &Connection, id: &str) -> Result<WorkPlan> {
    let r = object_refs::get(conn, ObjectKind::Plan, number(id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", id)))?;
    project(conn, &r, &asks::load(conn, &r.crew_id)?)
}
pub fn list(conn: &Connection, p: WorkListParams) -> Result<WorkPage> {
    let limit = p.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadLimit)));
    }
    let after = p.after.as_deref().map(number).transpose()?.unwrap_or(0);
    if let Some(crew) = &p.crew_id {
        crews::get(conn, crew)?;
    }
    let mut stmt = conn.prepare("SELECT number,crew_id,source_id,created_at FROM object_refs WHERE kind='plan' AND number>?1 AND (?2 IS NULL OR crew_id=?2) ORDER BY number LIMIT ?3")?;
    let mut refs = stmt
        .query_map(params![after, p.crew_id, i64::from(limit) + 1], |r| {
            Ok(ObjectReference {
                number: r.get(0)?,
                kind: ObjectKind::Plan,
                crew_id: r.get(1)?,
                source_id: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = refs.len() > usize::from(limit);
    refs.truncate(usize::from(limit));
    let next_after = if more {
        refs.last().map(ObjectReference::id)
    } else {
        None
    };
    let mut books = BTreeMap::new();
    let mut items = Vec::with_capacity(refs.len());
    for r in refs {
        if !books.contains_key(&r.crew_id) {
            books.insert(r.crew_id.clone(), asks::load(conn, &r.crew_id)?);
        }
        items.push(project(conn, &r, &books[&r.crew_id])?.into());
    }
    Ok(WorkPage { items, next_after })
}
fn actor(conn: &Connection, by: &crate::Sender, crew: &str) -> Result<crewd_work::Actor> {
    use crewd_work::Actor;
    match by {
        crate::Sender::Human => Ok(Actor::Human),
        crate::Sender::System => Err(CollabError::Forbidden(Refusal::new(
            Code::WorkActorForbidden,
        ))),
        crate::Sender::Session(id) => {
            let session = crate::sessions::get(conn, id)?;
            crews::refuse_archived(conn, &session.crew_id)?;
            if crews::acts_as_captain_of(conn, id, crew)? {
                Ok(Actor::captain(id))
            } else if session.crew_id == crew && session.role == crewd_proto::Role::Admin {
                Ok(Actor::admin(id))
            } else {
                Ok(Actor::worker(id))
            }
        }
    }
}
fn timestamp(value: Option<&str>) -> Result<Option<crewd_work::Ts>> {
    value
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v)
                .map(|t| crewd_work::Ts(t.timestamp_millis()))
                .map_err(|_| CollabError::Invalid(Refusal::new(Code::WorkBadTime).arg("value", v)))
        })
        .transpose()
}
fn local_reference(conn: &Connection, kind: ObjectKind, crew: &str, id: &str) -> Result<String> {
    let n = id
        .strip_prefix(kind.prefix())
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|n| *n > 0 && id == format!("{}{n}", kind.prefix()))
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadId).arg("input", id)))?;
    let r = object_refs::get(conn, kind, n)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", id)))?;
    if r.crew_id != crew {
        return Err(CollabError::Invalid(
            Refusal::new(Code::WorkLinkOtherCrew).arg("id", id),
        ));
    }
    r.source_id
        .strip_prefix("work:")
        .map(str::to_owned)
        .ok_or_else(|| invalid("work command reference source is invalid"))
}
fn local_plan(conn: &Connection, crew: &str, id: &str) -> Result<crewd_work::WorkId> {
    local_reference(conn, ObjectKind::Plan, crew, id)?
        .parse()
        .map_err(|_| invalid("work command plan identity is invalid"))
}
fn changed(tx: &mut crate::Tx<'_>, by: &crate::Sender, crew: &str, id: &str) -> Result<()> {
    if let crate::Sender::Session(session) = by {
        crate::sessions::note_progress(tx, session, crate::ProgressKind::WorkItem)?;
    }
    tx.note(crate::Change::WorkChanged {
        crew_id: crew.into(),
        id: id.into(),
    });
    Ok(())
}
pub fn create(
    tx: &mut crate::Tx<'_>,
    by: &crate::Sender,
    p: crewd_proto::WorkCreateParams,
) -> Result<WorkPlan> {
    use crewd_work::{Body, ItemRef, NewItem, SessionId};
    crews::refuse_archived(tx.conn(), &p.crew_id)?;
    let actor = actor(tx.conn(), by, &p.crew_id)?;
    let owner = p.owner.or_else(|| actor.session().map(ToString::to_string));
    if let Some(owner) = &owner {
        let owner = crate::sessions::get(tx.conn(), owner)?;
        crews::refuse_archived(tx.conn(), &owner.crew_id)?;
    }
    let item = NewItem {
        title: p.title,
        body: Body {
            goal: p.goal,
            acceptance: p.acceptance,
            level: match p.level {
                crewd_proto::WorkLevel::Local => DoneLevel::Local,
                crewd_proto::WorkLevel::Public => DoneLevel::Public,
                crewd_proto::WorkLevel::Production => DoneLevel::Production,
            },
        },
        owner: owner.map(|id| SessionId::new(&id)),
        parent: p
            .parent
            .as_deref()
            .map(|id| local_plan(tx.conn(), &p.crew_id, id).map(ItemRef::Id))
            .transpose()?,
        depends_on: p
            .depends_on
            .iter()
            .map(|id| local_plan(tx.conn(), &p.crew_id, id).map(ItemRef::Id))
            .collect::<Result<_>>()?,
        sources: p
            .sources
            .iter()
            .map(|id| {
                local_reference(tx.conn(), ObjectKind::Requirement, &p.crew_id, id)?
                    .parse()
                    .map_err(|_| invalid("work command requirement identity is invalid"))
            })
            .collect::<Result<_>>()?,
        next_check_at: timestamp(p.next_check_at.as_deref())?,
        channel: None,
    };
    let local = asks::with_book(tx, &p.crew_id, |b, now| {
        b.create_item(&actor, now, item).map(|a| a.value)
    })?;
    let id = reference(tx.conn(), ObjectKind::Plan, &p.crew_id, local)?;
    changed(tx, by, &p.crew_id, &id)?;
    get(tx.conn(), &id)
}
pub fn update(
    tx: &mut crate::Tx<'_>,
    by: &crate::Sender,
    p: crewd_proto::WorkUpdateParams,
) -> Result<WorkPlan> {
    use crewd_proto::WorkUpdateStatus as S;
    use crewd_work::{Progress, Report};
    let n = number(&p.id)?;
    let r = object_refs::get(tx.conn(), ObjectKind::Plan, n)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    crews::refuse_archived(tx.conn(), &r.crew_id)?;
    let actor = actor(tx.conn(), by, &r.crew_id)?;
    let local = local_plan(tx.conn(), &r.crew_id, &p.id)?;
    let revision = p
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|n| p.expected_revision == n.to_string())
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    let done = p.status == Some(S::Done);
    if (!done && (p.evidence.is_some() || p.note.as_ref().is_none_or(|n| n.trim().is_empty())))
        || (done && p.next_check_at.is_some())
    {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
    }
    let next = timestamp(p.next_check_at.as_deref())?;
    let evidence = p.evidence.map(|e| match e.kind {
        crewd_proto::WorkEvidenceKind::Commit => Evidence::Commit(e.value),
        crewd_proto::WorkEvidenceKind::Url => Evidence::Url(e.value),
        crewd_proto::WorkEvidenceKind::Text => Evidence::Text(e.value),
    });
    // The revision comparison and command run inside the same single-writer transaction.
    let current = asks::load(tx.conn(), &r.crew_id)?;
    if current.item(local).is_none_or(|i| i.revision != revision) {
        return Err(CollabError::Conflict(
            Refusal::new(Code::WorkRevisionConflict).arg("id", &p.id),
        ));
    }
    asks::with_book(tx, &r.crew_id, |b, now| {
        if !done || p.note.is_some() {
            b.report(
                &actor,
                now,
                local,
                Report {
                    status: match p.status {
                        Some(S::Open) => Some(Progress::Open),
                        Some(S::Active) => Some(Progress::Active),
                        _ => None,
                    },
                    next_check_at: next,
                    note: p.note.unwrap_or_default(),
                },
            )?;
        }
        if done {
            b.mark_done(&actor, now, local, evidence)?;
        }
        Ok(())
    })?;
    changed(tx, by, &r.crew_id, &p.id)?;
    get(tx.conn(), &p.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};
    use crewd_work::{Actor, ItemRef, NewItem};
    fn create(s: &mut crewd_store::Store, crew: &str, title: &str) -> String {
        ok(s, |tx| {
            asks::with_book(tx, crew, |b, now| {
                b.create_item(&Actor::Human, now, NewItem::titled(title))
            })
        });
        list(
            s.conn(),
            WorkListParams {
                crew_id: Some(crew.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .items
        .last()
        .unwrap()
        .id
        .clone()
    }
    #[test]
    fn global_ids_do_not_alias_local_ids_and_pagination_spans_crews() {
        let mut s = store();
        let a = create(&mut s, "crew-a", "first");
        let b = create(&mut s, "crew-b", "second");
        assert_ne!(a, b);
        let first = list(
            s.conn(),
            WorkListParams {
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(first.items[0].title, "first");
        assert_eq!(first.next_after, Some(a.clone()));
        let second = list(
            s.conn(),
            WorkListParams {
                after: first.next_after,
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(second.items[0].title, "second");
        assert_eq!(second.next_after, None);
        assert_eq!(get(s.conn(), &b).unwrap().crew_id, "crew-b");
        s.conn()
            .execute(
                "UPDATE crews SET archived_at='removed' WHERE id='crew-b'",
                [],
            )
            .unwrap();
        assert_eq!(get(s.conn(), &b).unwrap().title, "second");
        assert_eq!(
            list(s.conn(), WorkListParams::default())
                .unwrap()
                .items
                .len(),
            2
        );
    }
    #[test]
    fn links_are_global_and_large_identifiers_stay_exact_strings() {
        let mut s = store();
        s.conn()
            .execute(
                "INSERT INTO sqlite_sequence(name,seq) VALUES('object_refs',9007199254740992)",
                [],
            )
            .unwrap();
        let parent = create(&mut s, "crew-a", "parent");
        create(&mut s, "crew-b", "different crew local W1");
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.create_item(
                    &Actor::Human,
                    now,
                    NewItem::titled("child").under(ItemRef::Id(crewd_work::WorkId(1))),
                )
            })
        });
        let page = list(
            s.conn(),
            WorkListParams {
                crew_id: Some("crew-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.items[1].parent, Some(parent.clone()));
        assert_eq!(parent, "W9007199254740993");
        let wire = serde_json::to_value(&page).unwrap();
        assert_eq!(wire["items"][0]["id"], parent);
        assert!(wire["items"][0]["revision"].is_string());
        assert!(wire["items"][0].get("goal").is_none());
        assert!(wire["items"][0].get("acceptance").is_none());
    }
    #[test]
    fn malformed_queries_and_missing_authority_are_refused() {
        let mut s = store();
        for id in ["W0", "W01", "H1", "W+1", "W9223372036854775808"] {
            assert!(matches!(get(s.conn(), id), Err(CollabError::Invalid(_))));
        }
        assert!(matches!(
            get(s.conn(), "W999"),
            Err(CollabError::NotFound(_))
        ));
        for limit in [0, 201] {
            assert!(matches!(
                list(
                    s.conn(),
                    WorkListParams {
                        limit: Some(limit),
                        ..Default::default()
                    }
                ),
                Err(CollabError::Invalid(_))
            ));
        }
        let id = create(&mut s, "crew-a", "real");
        s.conn()
            .execute("DELETE FROM work_books WHERE crew_id='crew-a'", [])
            .unwrap();
        assert!(matches!(get(s.conn(), &id), Err(CollabError::Store(_))));
        assert!(matches!(
            list(s.conn(), WorkListParams::default()),
            Err(CollabError::Store(_))
        ));
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, store};
    use crate::{Change, ProgressKind, Sender};
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    fn input() -> WorkCreateParams {
        WorkCreateParams {
            crew_id: "crew-a".into(),
            title: "synthetic plan".into(),
            ..Default::default()
        }
    }
    fn finish(p: &WorkPlan) -> WorkUpdateParams {
        WorkUpdateParams {
            id: p.id.clone(),
            expected_revision: p.revision.clone(),
            status: Some(WorkUpdateStatus::Done),
            ..Default::default()
        }
    }
    #[test]
    fn owned_work_reports_real_progress_and_completes_without_evidence() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let (p, changes) = crate::run(&mut s, NOW, |tx| create(tx, &by, input())).unwrap();
        assert_eq!(p.owner.as_deref(), Some("w-a1"));
        assert!(changes.contains(&Change::Progress {
            session_id: "w-a1".into(),
            kind: ProgressKind::WorkItem
        }));
        assert!(changes.contains(&Change::WorkChanged {
            crew_id: "crew-a".into(),
            id: p.id.clone()
        }));
        let active = ok(&mut s, |tx| {
            update(
                tx,
                &by,
                WorkUpdateParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    status: Some(WorkUpdateStatus::Active),
                    note: Some("next concrete step".into()),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 60_000)),
                    ..Default::default()
                },
            )
        });
        assert_eq!(active.status, "active");
        assert_eq!(active.note.as_deref(), Some("next concrete step"));
        let done = ok(&mut s, |tx| update(tx, &by, finish(&active)));
        assert_eq!(done.status, "done");
        assert!(done.evidence.is_none());
        assert!(done.next_check_at.is_none());
        assert_eq!(
            crewd_store::work_events::list(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            3
        );
    }
    #[test]
    fn stale_revision_and_unauthorized_actor_leave_all_state_unchanged() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        assert!(matches!(
            err(&mut s, |tx| update(
                tx,
                &Sender::Session("w-a2".into()),
                finish(&p)
            )),
            CollabError::Work(crewd_work::Rejected::Forbidden(_))
        ));
        assert!(matches!(
            err(&mut s, |tx| update(
                tx,
                &Sender::Session("cap-b".into()),
                finish(&p)
            )),
            CollabError::Work(crewd_work::Rejected::Forbidden(_))
        ));
        assert_eq!(get(s.conn(), &p.id).unwrap(), p);
        let done = ok(&mut s, |tx| update(tx, &by, finish(&p)));
        let stale = err(&mut s, |tx| update(tx, &by, finish(&p)));
        assert!(matches!(stale, CollabError::Conflict(_)));
        assert_eq!(get(s.conn(), &p.id).unwrap(), done);
        assert_eq!(
            crewd_store::work_events::list(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn stale_report_cannot_overwrite_a_newer_next_step() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let original = ok(&mut s, |tx| create(tx, &by, input()));
        let update_request = |note: &str| WorkUpdateParams {
            id: original.id.clone(),
            expected_revision: original.revision.clone(),
            note: Some(note.into()),
            ..Default::default()
        };
        let current = ok(&mut s, |tx| {
            update(tx, &by, update_request("newer next step"))
        });
        let e = err(&mut s, |tx| {
            update(tx, &by, update_request("stale next step"))
        });
        assert_eq!(e.refusal().unwrap().code, Code::WorkRevisionConflict);
        assert_eq!(get(s.conn(), &original.id).unwrap(), current);
        assert_eq!(
            crewd_store::work_events::list(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn target_captain_and_management_captain_use_current_authority() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        let done = ok(&mut s, |tx| {
            update(tx, &Sender::Session("cap-a".into()), finish(&p))
        });
        assert_eq!(done.status, "done");
        let chief = crate::testkit::org_captain(&mut s);
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        assert_eq!(
            ok(&mut s, |tx| update(tx, &Sender::Session(chief), finish(&p))).status,
            "done"
        );
    }
    #[test]
    fn failed_progress_write_rolls_back_work_events_and_issued_references() {
        let mut s = store();
        s.conn().execute_batch("CREATE TRIGGER fail_work_progress BEFORE UPDATE OF last_progress_at ON sessions BEGIN SELECT RAISE(ABORT,'synthetic progress failure'); END;").unwrap();
        assert!(matches!(
            err(&mut s, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                input()
            )),
            CollabError::Store(_)
        ));
        assert!(
            list(s.conn(), WorkListParams::default())
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            crewd_store::work_events::list(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(
            s.conn()
                .query_row(
                    "SELECT last_progress_at FROM sessions WHERE id='w-a1'",
                    [],
                    |r| r.get::<_, Option<String>>(0)
                )
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn frozen_crews_reject_writes_and_cross_crew_links_never_alias() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        let mut cross = input();
        cross.crew_id = "crew-b".into();
        cross.parent = Some(p.id.clone());
        let e = err(&mut s, |tx| create(tx, &Sender::Human, cross));
        assert_eq!(e.refusal().unwrap().code, Code::WorkLinkOtherCrew);
        s.conn()
            .execute(
                "UPDATE crews SET archived_at='frozen' WHERE id='crew-a'",
                [],
            )
            .unwrap();
        let e = err(&mut s, |tx| update(tx, &Sender::Human, finish(&p)));
        assert_eq!(e.refusal().unwrap().code, Code::CrewRemoved);
        assert_eq!(get(s.conn(), &p.id).unwrap(), p);
    }
}
