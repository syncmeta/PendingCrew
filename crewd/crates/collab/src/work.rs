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
