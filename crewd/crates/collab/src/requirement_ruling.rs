//! Explicit business ruling for the exact dropped direct work of an original requirement.
use crate::{
    Change, CollabError, Result, Sender, Tx, asks, crews, judgments,
    refusal::{Code, Refusal},
    requirements, resource_acl, work,
};
use crewd_proto::{
    ManualTodoState, RequirementRuleCloseParams, RequirementRulingBy, RequirementRulingCheck,
    RequirementRulingCheckParams, RequirementRulingDrop, RequirementRulingReceipt,
    RequirementRulingResult, RequirementRulingState, RequirementRulingWork, TodoCloseParams,
};
use crewd_store::{StoreError, meta_get, meta_set, object_refs::ObjectKind};
use crewd_work::{Actor, DropKind, EntryNo, EntryStatus, Event, EventKind, Status, WorkBook};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const MAX_WORK: usize = 32;
pub const CAPTAIN_CHECK_TOOL: &str = "todo_ruling_check";
pub const CAPTAIN_CLOSE_TOOL: &str = "todo_ruling_close";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    receipt: RequirementRulingReceipt,
    intent: String,
    event: String,
    seal: String,
}
impl Saved {
    fn seal(&self) -> Result<String> {
        judgments::fingerprint(&(&self.receipt, &self.intent, &self.event))
    }
}

fn invalid() -> CollabError {
    StoreError::Invalid("invalid authoritative requirement ruling evidence".into()).into()
}

fn denied() -> CollabError {
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}

fn conflict(request: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ResourceRequestConflict).arg("request", request))
}
fn needs_review(id: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::TodoRulingReviewRequired).arg("id", id))
}

fn request_key(id: &str) -> String {
    format!("requirement.ruling.request.v1:{id}")
}

fn result_key(id: &str) -> String {
    format!("requirement.ruling.result.v1:{id}")
}
fn canonical_revision(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
fn close_revision(by: &Sender, revision: &str) -> Result<String> {
    let captain = match by {
        Sender::Session(id) => Some(id),
        _ => None,
    };
    judgments::fingerprint(&("requirement.ruling.close.v1", captain, revision))
}

fn require_actor(c: &Connection, crew: &str, by: &Sender) -> Result<()> {
    match by {
        Sender::Human => Ok(()),
        Sender::Session(captain) if crews::acts_as_captain_of(c, captain, crew)? => Ok(()),
        _ => Err(denied()),
    }
}

pub fn subject_crew(c: &Connection, id: &str) -> Result<String> {
    Ok(requirements::reference(c, id)?.crew_id)
}

fn check_book(
    c: &Connection,
    by: &Sender,
    p: &RequirementRulingCheckParams,
    book: &WorkBook,
) -> Result<(RequirementRulingCheck, EntryNo)> {
    let crew = &p.crew_id;
    let id = &p.id;
    require_actor(c, crew, by)?;
    if p.after.is_some() != p.expected_revision.is_some() {
        return Err(resource_acl::invalid("after"));
    }
    if let Some(after) = &p.after {
        work::number(after)?;
    }
    if p.expected_revision
        .as_deref()
        .is_some_and(|r| !canonical_revision(r))
    {
        return Err(resource_acl::invalid("expectedRevision"));
    }
    let reference = requirements::reference(c, id)?;
    if reference.crew_id != *crew {
        return Err(denied());
    }
    let no = requirements::core(c, &reference)?;
    let detail = requirements::detail(c, &reference, book)?;
    let linked = || {
        book.state()
            .items
            .values()
            .filter(|item| item.sources.contains(&no))
    };
    let count = linked().count();
    let mut view = RequirementRulingCheck {
        crew_id: crew.clone(),
        id: id.clone(),
        state: RequirementRulingState::NotNeedsRuling,
        revision: None,
        close_revision: None,
        linked_work_count: count,
        work: Vec::new(),
        next_after: None,
    };
    if detail.derived_status != "needs_ruling"
        || detail.todo_attributes.manual_state == Some(ManualTodoState::Done)
        || detail.todo.deleted_at.is_some()
        || crews::is_archived(c, crew)?
        || crews::get(c, crew)?.removed_at.is_some()
    {
        if p.expected_revision.is_some() {
            view.state = RequirementRulingState::Stale;
        }
        return Ok((view, no));
    }
    view.state = RequirementRulingState::ReviewIncomplete;
    let mut rows = Vec::with_capacity(count.min(MAX_WORK));
    let mut linked_evidence = ring::digest::Context::new(&ring::digest::SHA256);
    let mut after_seen = p.after.is_none();
    let mut valid_boundary = after_seen;
    let mut has_next = false;
    for (index, item) in linked().enumerate() {
        let target = crewd_store::work_organization::reference_for_core(
            c,
            ObjectKind::Plan,
            &item.id.to_string(),
        )?;
        let Some(target) = target.filter(|r| {
            book.state()
                .item_crew(item.id)
                .is_some_and(|c| c.as_str() == r.crew_id)
        }) else {
            return Ok((view, no));
        };
        if let Sender::Session(captain) = by
            && (!crews::acts_as_captain_of(c, captain, &target.crew_id)?
                || match &item.channel {
                    Some(ch) => !crate::channels::readable_by(c, ch.as_str(), captain)?,
                    None => false,
                })
        {
            view.linked_work_count = 0;
            return Ok((view, no));
        }
        let Some(drop) = item
            .drop_reason
            .as_ref()
            .filter(|d| !d.note.trim().is_empty() && item.status == Status::Dropped)
        else {
            return Ok((view, no));
        };
        let id = target.id();
        linked_evidence.update(
            judgments::fingerprint(&(item.id, &id, &target.crew_id, item.revision, drop))?
                .as_bytes(),
        );
        if p.after.as_deref() == Some(id.as_str()) {
            valid_boundary = (index + 1) % MAX_WORK == 0 && index + 1 < count;
            after_seen = true;
            continue;
        }
        if !after_seen {
            continue;
        }
        if rows.len() == MAX_WORK {
            has_next = true;
            continue;
        }
        let mut note = drop.note.chars();
        let drop_note = note.by_ref().take(500).collect();
        rows.push(RequirementRulingWork {
            id,
            crew_id: target.crew_id,
            title: item.title.chars().take(200).collect(),
            revision: item.revision.to_string(),
            drop_note,
            drop_note_truncated: note.next().is_some(),
            drop_kind: match drop.kind {
                DropKind::Duplicate => RequirementRulingDrop::Duplicate,
                DropKind::Superseded => RequirementRulingDrop::Superseded,
                DropKind::NotNeeded => RequirementRulingDrop::NotNeeded,
            },
        });
    }
    if count == 0 {
        return Ok((view, no));
    }
    // A foreign direct link can be created and purged between reads. The retained
    // organization event watermark prevents that old witness from becoming current again.
    let fingerprint = judgments::fingerprint(&(
        crew,
        id,
        no,
        &detail.todo.id,
        &detail.todo.text,
        &detail.todo.created_at,
        &detail.todo_attributes,
        linked_evidence.finish().as_ref(),
        book.state().last_seq,
        crewd_store::judgment_state::epoch(c)?,
    ))?;
    if p.expected_revision
        .as_ref()
        .is_some_and(|expected| expected != &fingerprint)
    {
        view.state = RequirementRulingState::Stale;
        return Ok((view, no));
    }
    if !valid_boundary || rows.is_empty() {
        return Err(resource_acl::invalid("after"));
    }
    if !has_next {
        view.close_revision = Some(close_revision(by, &fingerprint)?);
    }
    view.revision = Some(fingerprint);
    view.state = RequirementRulingState::Ready;
    view.next_after = has_next.then(|| rows.last().expect("full page").id.clone());
    view.work = rows;
    Ok((view, no))
}

pub fn check(
    c: &Connection,
    by: &Sender,
    p: &RequirementRulingCheckParams,
) -> Result<RequirementRulingCheck> {
    let book = asks::load_org(c)?;
    check_book(c, by, p, &book).map(|(view, _)| view)
}

fn event(c: &Connection, crew: &str, seq: &str) -> Result<Event> {
    let n = seq
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == seq)
        .ok_or_else(invalid)?;
    let raw: String = c
        .query_row(
            "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
            (n, crew),
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::from)?
        .ok_or_else(invalid)?;
    let event: Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if event.seq != n as u64 {
        return Err(invalid());
    }
    Ok(event)
}

fn verify(c: &Connection, saved: &Saved, crew: &str, id: &str, request: &str) -> Result<()> {
    let r = &saved.receipt;
    let reference = requirements::reference(c, id)?;
    if reference.crew_id != crew
        || r.crew_id != crew
        || r.id != id
        || r.request_id != request
        || resource_acl::uuid(request)? != request
        || saved.seal()? != saved.seal
    {
        return Err(invalid());
    }
    let no = requirements::core(c, &reference)?;
    let e = event(c, crew, &r.event_seq)?;
    let by = match r.by {
        RequirementRulingBy::Human => r.captain.is_none() && e.by == Actor::Human,
        RequirementRulingBy::Captain => {
            r.captain.is_some() && e.by.session().map(|s| s.as_str()) == r.captain.as_deref()
        }
    };
    let book = asks::load_org(c)?;
    if !by
        || e.at.0 != r.closed_at_ms
        || judgments::fingerprint(&e)? != saved.event
        || !matches!(&e.kind, EventKind::EntryClosed {entry, note} if *entry == no && note == &r.note)
        || !book
            .entry(no)
            .and_then(|entry| entry.closed.as_ref())
            .is_some_and(|closed| {
                closed.by == e.by
                    && closed.at == e.at
                    && closed.note == r.note
                    && closed.evidence.is_none()
            })
    {
        return Err(invalid());
    }
    Ok(())
}

fn read(c: &Connection, request: &str) -> Result<Option<Saved>> {
    meta_get(c, &request_key(request))?
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(StoreError::from)
                .map_err(Into::into)
        })
        .transpose()
}

pub fn result(c: &Connection, crew: &str, id: &str) -> Result<RequirementRulingResult> {
    if requirements::reference(c, id)?.crew_id != crew {
        return Err(denied());
    }
    let receipt = if let Some(request) = meta_get(c, &result_key(id))? {
        let saved = read(c, &request)?.ok_or_else(invalid)?;
        verify(c, &saved, crew, id, &request)?;
        Some(saved.receipt)
    } else {
        None
    };
    Ok(RequirementRulingResult {
        crew_id: crew.into(),
        id: id.into(),
        receipt,
    })
}

pub fn close(
    tx: &mut Tx<'_>,
    by: &Sender,
    request: &str,
    p: RequirementRuleCloseParams,
) -> Result<RequirementRulingReceipt> {
    require_actor(tx.conn(), &p.crew_id, by)?;
    if requirements::reference(tx.conn(), &p.id)?.crew_id != p.crew_id {
        return Err(denied());
    }
    if resource_acl::uuid(request)? != request {
        return Err(resource_acl::invalid("requestId"));
    }
    if !canonical_revision(&p.expected_revision) {
        return Err(resource_acl::invalid("expectedRevision"));
    }
    let note = p.note.trim();
    if note.is_empty() || p.note.len() > 4096 {
        return Err(resource_acl::invalid("note"));
    }
    let captain = match by {
        Sender::Session(id) => Some(id.clone()),
        _ => None,
    };
    let intent = judgments::fingerprint(&(&captain, &p))?;
    if let Some(saved) = read(tx.conn(), request)? {
        // The public Human journal already handles same-principal exact retry.
        // A new journal principal may not acquire another Human's old request ID.
        if captain.is_none()
            || saved.intent != intent
            || meta_get(tx.conn(), &result_key(&p.id))?.as_deref() != Some(request)
        {
            return Err(conflict(request));
        }
        verify(tx.conn(), &saved, &p.crew_id, &p.id, request)?;
        return Ok(saved.receipt);
    }
    if meta_get(tx.conn(), &result_key(&p.id))?.is_some() {
        return Err(needs_review(&p.id));
    }
    let book = asks::load_org(tx.conn())?;
    let (check, no) = check_book(
        tx.conn(),
        by,
        &RequirementRulingCheckParams {
            crew_id: p.crew_id.clone(),
            id: p.id.clone(),
            after: None,
            expected_revision: None,
        },
        &book,
    )?;
    if check
        .revision
        .map(|revision| close_revision(by, &revision))
        .transpose()?
        .as_deref()
        != Some(p.expected_revision.as_str())
    {
        return Err(needs_review(&p.id));
    }
    let actor = work::actor(tx.conn(), by, &p.crew_id)?;
    let seq = book
        .state()
        .last_seq
        .checked_add(1)
        .ok_or_else(invalid)?
        .to_string();
    asks::with_book(tx, &p.crew_id, |b, now| {
        b.close_entry(&actor, now, no, note, None)
    })
    .map_err(work::command_refusal)?;
    let e = event(tx.conn(), &p.crew_id, &seq)?;
    let receipt = RequirementRulingReceipt {
        crew_id: p.crew_id,
        id: p.id,
        request_id: request.into(),
        expected_revision: p.expected_revision,
        note: note.into(),
        closed_at_ms: e.at.0,
        event_seq: seq,
        by: if captain.is_some() {
            RequirementRulingBy::Captain
        } else {
            RequirementRulingBy::Human
        },
        captain,
    };
    let mut saved = Saved {
        receipt,
        intent,
        event: judgments::fingerprint(&e)?,
        seal: String::new(),
    };
    saved.seal = saved.seal()?;
    verify(
        tx.conn(),
        &saved,
        &saved.receipt.crew_id,
        &saved.receipt.id,
        request,
    )?;
    meta_set(
        tx.conn(),
        &request_key(request),
        &serde_json::to_string(&saved).map_err(StoreError::from)?,
    )?;
    meta_set(tx.conn(), &result_key(&saved.receipt.id), request)?;
    tx.note(Change::WorkChanged {
        crew_id: saved.receipt.crew_id.clone(),
        id: saved.receipt.id.clone(),
    });
    Ok(saved.receipt)
}

/// Keep other Human closures compatible, but route the derived ruling through exact review.
pub fn close_legacy(tx: &mut Tx<'_>, p: TodoCloseParams) -> Result<crewd_proto::RequirementDetail> {
    let r = requirements::reference(tx.conn(), &p.id)?;
    let no = requirements::core(tx.conn(), &r)?;
    let book = asks::load_org(tx.conn())?;
    if book.entry_view(no).ok_or_else(invalid)?.status == EntryStatus::NeedsRuling {
        return Err(needs_review(&p.id));
    }
    requirements::close(tx, &Sender::Human, p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok, store};
    use crewd_proto::{TodoCreateParams, WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};

    fn requirement(s: &mut crewd_store::Store) -> String {
        ok(s, |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    text: "original synthetic requirement".into(),
                    channel_id: None,
                },
            )
        })
        .todo
        .requirement_id
        .unwrap()
    }

    fn dropped(s: &mut crewd_store::Store, id: &str, crew: &str) -> String {
        let plan = ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: "one attempted implementation".into(),
                    sources: vec![id.into()],
                    ..Default::default()
                },
            )
        });
        ok(s, |tx| {
            work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id: plan.id,
                    expected_revision: plan.revision,
                    status: Some(WorkUpdateStatus::Dropped),
                    note: Some("not viable for this requirement".into()),
                    ..Default::default()
                },
            )
        })
        .id
    }

    fn params(id: &str) -> RequirementRulingCheckParams {
        RequirementRulingCheckParams {
            crew_id: "crew-a".into(),
            id: id.into(),
            after: None,
            expected_revision: None,
        }
    }

    fn ruling(id: &str, revision: String) -> RequirementRuleCloseParams {
        RequirementRuleCloseParams {
            crew_id: "crew-a".into(),
            id: id.into(),
            expected_revision: revision,
            note: "  human-reviewed business decision  ".into(),
        }
    }

    #[test]
    fn real_foreign_evidence_is_human_visible_and_old_cut_cannot_revive_after_purge() {
        let mut s = store();
        let id = requirement(&mut s);
        let _local = dropped(&mut s, &id, "crew-a");
        let first = check(s.conn(), &Sender::Human, &params(&id)).unwrap();
        let before = first.close_revision.unwrap();
        let foreign = dropped(&mut s, &id, "crew-b");
        let changed = check(s.conn(), &Sender::Human, &params(&id)).unwrap();
        assert_eq!(changed.linked_work_count, 2);
        assert!(changed.work.iter().any(|row| row.id == foreign
            && row.crew_id == "crew-b"
            && row.drop_kind == RequirementRulingDrop::NotNeeded
            && row.drop_note == "not viable for this requirement"));
        let captain = Sender::Session("cap-a".into());
        let limited = check(s.conn(), &captain, &params(&id)).unwrap();
        assert_eq!(limited.state, RequirementRulingState::ReviewIncomplete);
        assert!(limited.work.is_empty() && limited.revision.is_none());
        let chief = ok(&mut s, |tx| {
            crate::sessions::register(
                tx,
                crate::sessions::NewSession {
                    crew_id: "chief".into(),
                    role: crewd_proto::Role::Captain,
                    executor: "claude".into(),
                    title: Some("management captain".into()),
                    ..Default::default()
                },
            )
        })
        .id;
        let global = check(s.conn(), &Sender::Session(chief), &params(&id)).unwrap();
        assert_eq!(global.state, RequirementRulingState::Ready);
        assert_eq!(global.linked_work_count, 2);

        ok(&mut s, |tx| crews::archive(tx, "crew-b"));
        ok(&mut s, |tx| crews::purge(tx, "crew-b"));
        let restored_set = check(s.conn(), &Sender::Human, &params(&id)).unwrap();
        assert_eq!(restored_set.linked_work_count, 1);
        assert_ne!(
            restored_set.close_revision.as_deref(),
            Some(before.as_str())
        );
        let request = uuid::Uuid::new_v4().to_string();
        assert!(
            matches!(crate::run(&mut s, NOW, |tx| close(tx, &Sender::Human, &request, ruling(&id, before))),
            Err(CollabError::Conflict(r)) if r.code == Code::TodoRulingReviewRequired)
        );
        assert!(result(s.conn(), "crew-a", &id).unwrap().receipt.is_none());
    }

    #[test]
    fn pagination_refuses_forgery_and_stale_close_is_atomic_and_preserves_work() {
        let mut s = store();
        let id = requirement(&mut s);
        for _ in 0..=MAX_WORK {
            dropped(&mut s, &id, "crew-a");
        }
        let first = check(s.conn(), &Sender::Human, &params(&id)).unwrap();
        assert_eq!(first.work.len(), MAX_WORK);
        assert!(first.close_revision.is_none());
        let cursor = first.next_after.clone().unwrap();
        let last = RequirementRulingCheckParams {
            after: Some(cursor),
            expected_revision: first.revision.clone(),
            ..params(&id)
        };
        let mut forged = last.clone();
        forged.after = Some(first.work[0].id.clone());
        assert!(check(s.conn(), &Sender::Human, &forged).is_err());
        let mut mixed = last.clone();
        mixed.expected_revision = None;
        assert!(check(s.conn(), &Sender::Human, &mixed).is_err());
        let terminal = check(s.conn(), &Sender::Human, &last).unwrap();
        assert_eq!(terminal.revision, first.revision);
        assert_eq!(terminal.work.len(), 1);
        assert!(terminal.next_after.is_none());
        let uuid = uuid::Uuid::new_v4().to_string();
        let wrong = ruling(&id, first.revision.unwrap());
        assert!(crate::run(&mut s, NOW, |tx| close(tx, &Sender::Human, &uuid, wrong)).is_err());
        let old_close = ruling(&id, terminal.close_revision.unwrap());
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert_eq!(
            check(s.conn(), &Sender::Human, &last).unwrap().state,
            RequirementRulingState::Stale
        );
        assert!(
            crate::run(&mut s, NOW, |tx| close(
                tx,
                &Sender::Human,
                &uuid,
                old_close
            ))
            .is_err()
        );
        let fresh = check(s.conn(), &Sender::Human, &params(&id)).unwrap();
        let last = check(
            s.conn(),
            &Sender::Human,
            &RequirementRulingCheckParams {
                after: fresh.next_after,
                expected_revision: fresh.revision,
                ..params(&id)
            },
        )
        .unwrap();
        let p = ruling(&id, last.close_revision.unwrap());
        let before = asks::load_org(s.conn()).unwrap().state().clone();
        s.conn().execute_batch("CREATE TRIGGER deny_ruling BEFORE INSERT ON meta WHEN NEW.key LIKE 'requirement.ruling.result.v1:%' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| close(
                tx,
                &Sender::Human,
                &uuid,
                p.clone()
            ))
            .is_err()
        );
        assert_eq!(asks::load_org(s.conn()).unwrap().state(), &before);
        assert!(result(s.conn(), "crew-a", &id).unwrap().receipt.is_none());
        s.conn().execute_batch("DROP TRIGGER deny_ruling").unwrap();
        let saved = ok(&mut s, |tx| close(tx, &Sender::Human, &uuid, p.clone()));
        assert_eq!(saved.note, "human-reviewed business decision");
        assert_eq!(saved.by, RequirementRulingBy::Human);
        assert_eq!(
            result(s.conn(), "crew-a", &id).unwrap().receipt,
            Some(saved)
        );
        let after = asks::load_org(s.conn()).unwrap();
        assert_eq!(after.state().items, before.items);
        assert_eq!(after.state().last_seq, before.last_seq + 1);
        assert!(crate::run(&mut s, NOW, |tx| close(tx, &Sender::Human, &uuid, p)).is_err());
    }
}
