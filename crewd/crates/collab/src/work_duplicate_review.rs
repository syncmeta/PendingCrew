//! Exact, same-crew archive leads and explicit Human closure of pristine duplicates.
use crate::{
    CollabError, Result, Sender, Tx, asks, crews, judgments,
    refusal::{Code, Refusal},
    resource_acl, work,
};
use crewd_proto::{
    ChannelWorkKind, ChannelWorkObject, TodoOrigin, WorkCloseParams, WorkClosureHistoryPage,
    WorkClosureHistoryParams, WorkClosureKind, WorkDuplicateCandidate, WorkDuplicateCloseParams,
    WorkDuplicateClosure, WorkDuplicateHistoryParams, WorkDuplicatesPage, WorkDuplicatesParams,
    WorkEvidenceKind, WorkEvidenceParam,
};
use crewd_store::{StoreError, judgment_state, meta_get, meta_set, object_refs::ObjectKind};
use crewd_work::{
    Actor, AskState, Blocker, DropKind, Event, EventKind, Evidence, Status, WorkBook, WorkId,
    WorkItem,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SOURCES: usize = 8;
const MAX_LINKS: usize = 8;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_DEFINITION_BYTES: usize = 16_384;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    closure: WorkDuplicateClosure,
    checksum: String,
}

#[derive(Serialize)]
struct Link {
    channel: String,
    tags: String,
    generation: Option<String>,
}

struct Index<'a> {
    book: &'a WorkBook,
    crew: &'a str,
    epoch: String,
    issued: BTreeMap<i64, WorkId>,
    links: BTreeMap<i64, Option<Vec<Link>>>,
    referenced: BTreeSet<WorkId>,
}

fn invalid() -> CollabError {
    StoreError::Invalid("invalid exact duplicate work review authority".into()).into()
}

fn changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::WorkDuplicateCandidateChanged))
}

fn positive_revision(raw: &str) -> bool {
    raw.parse::<u64>()
        .is_ok_and(|n| n > 0 && n.to_string() == raw)
}

fn exact_next(previous: &str, current: &str) -> bool {
    positive_revision(previous)
        && previous
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_add(1))
            .is_some_and(|next| next.to_string() == current)
}

fn valid_text(raw: &str) -> bool {
    !raw.trim().is_empty() && raw.len() <= MAX_TEXT_BYTES
}

fn digest(raw: &str) -> bool {
    raw.len() == 64 && raw.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn scoped_links(c: &Connection, crew: &str) -> Result<BTreeMap<i64, Option<Vec<Link>>>> {
    let mut stmt = c.prepare(
        "SELECT r.number,l.channel_id,l.tags,l.source_crew_id,ch.crew_id,ch.kind,ch.archived_at
         FROM object_refs r JOIN channel_work_links l
           ON l.object_kind='work' AND l.object_id='W'||r.number
         LEFT JOIN channels ch ON ch.id=l.channel_id
         WHERE r.kind='plan' AND r.crew_id=?1 ORDER BY r.number,l.channel_id",
    )?;
    let mut links = BTreeMap::<i64, Option<Vec<Link>>>::new();
    let mut generations: BTreeMap<String, Option<String>> = BTreeMap::new();
    let rows = stmt.query_map([crew], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    for row in rows {
        let (number, channel, tags, source, owner, kind, archived) = row?;
        let entry = links.entry(number).or_insert_with(|| Some(Vec::new()));
        let Some(qualified) = entry.as_mut() else {
            continue;
        };
        if source != crew
            || owner.as_deref() != Some(crew)
            || archived.is_some()
            || !matches!(kind.as_deref(), Some("topic" | "lobby"))
            || qualified.len() == MAX_LINKS
        {
            *entry = None;
            continue;
        }
        let generation = match generations.entry(channel.clone()) {
            std::collections::btree_map::Entry::Occupied(row) => row.get().clone(),
            std::collections::btree_map::Entry::Vacant(row) => row
                .insert(crate::channel_archive::generation(c, &channel)?)
                .clone(),
        };
        qualified.push(Link {
            channel,
            tags,
            generation,
        });
    }
    Ok(links)
}

impl<'a> Index<'a> {
    fn load(c: &Connection, crew: &'a str, book: &'a WorkBook) -> Result<Self> {
        let mut stmt = c.prepare(
            "SELECT number,core_id FROM object_refs WHERE kind='plan' AND crew_id=?1 ORDER BY number",
        )?;
        let issued = stmt
            .query_map([crew], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (number, raw) = row?;
                let core: WorkId = raw.parse().map_err(|_| invalid())?;
                if number <= 0
                    || book
                        .state()
                        .item_crew(core)
                        .is_none_or(|v| v.as_str() != crew)
                {
                    return Err(invalid());
                }
                Ok((number, core))
            })
            .collect::<Result<_>>()?;
        // Include references from other crews. Closing this item must not
        // change a foreign work dependency or open Human question.
        let mut referenced = BTreeSet::new();
        for item in book
            .state()
            .items
            .values()
            .filter(|item| item.status.is_live())
        {
            referenced.extend(item.parent);
            referenced.extend(item.depends_on.iter().copied());
            referenced.extend(item.blocked_on.iter().filter_map(|blocker| match blocker {
                Blocker::Item(id) => Some(*id),
                Blocker::Human(_) => None,
            }));
        }
        referenced.extend(
            book.state()
                .asks
                .values()
                .filter_map(|ask| (ask.state == AskState::Open).then_some(ask.item).flatten()),
        );
        Ok(Self {
            book,
            crew,
            issued,
            referenced,
            epoch: judgment_state::epoch(c)?,
            links: scoped_links(c, crew)?,
        })
    }

    fn definition(&self, number: i64, item: &WorkItem) -> Result<Option<String>> {
        if !matches!(item.status, Status::Open | Status::Active)
            || item.pending_transfer.is_some()
            || item.parent.is_some()
            || !item.depends_on.is_empty()
            || !item.blocked_on.is_empty()
            // Old embedded channel links are outside this initial qualifier.
            || item.channel.is_some()
            || item.sources.is_empty()
            || item.sources.len() > MAX_SOURCES
            || item.body.goal.trim().is_empty()
            || item.body.acceptance.trim().is_empty()
            || item.title.len() + item.body.goal.len() + item.body.acceptance.len() > MAX_DEFINITION_BYTES
        {
            return Ok(None);
        }
        let links = match self.links.get(&number) {
            Some(Some(links)) => links.as_slice(),
            Some(None) => return Ok(None),
            None => &[],
        };
        let mut sources = item.sources.clone();
        sources.sort_unstable();
        sources.dedup();
        if sources.iter().any(|id| {
            self.book
                .state()
                .entry_crew(*id)
                .is_none_or(|crew| crew.as_str() != self.crew)
                || self
                    .book
                    .entry(*id)
                    .is_none_or(|entry| entry.closed.is_some() || entry.manual_done)
        }) {
            return Ok(None);
        }
        Ok(Some(judgments::fingerprint(&(
            &item.title,
            &item.body,
            sources,
            links,
        ))?))
    }

    fn discardable(&self, item: &WorkItem) -> bool {
        item.status == Status::Open
            && item.revision == 1
            && item.owner.is_none()
            && item.owner_history.is_empty()
            && item.next_check_at.is_none()
            && item.last_report.is_none()
            && !self.referenced.contains(&item.id)
    }

    fn make_candidate(
        &self,
        c: &Connection,
        duplicate: i64,
        survivor: i64,
        definition: &str,
    ) -> Result<WorkDuplicateCandidate> {
        let a = self
            .book
            .item(self.issued[&duplicate])
            .ok_or_else(invalid)?;
        let b = self.book.item(self.issued[&survivor]).ok_or_else(invalid)?;
        let mut source_ids = a.sources.iter().map(|id| {
            let r = crewd_store::work_organization::reference_for_core(c, ObjectKind::Requirement, &id.to_string())?
                .filter(|r| r.crew_id == self.crew).ok_or_else(invalid)?;
            let live: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM object_refs r JOIN todos t ON r.raw_todo_id=t.id
                 WHERE r.number=?1 AND t.crew_id=r.crew_id AND t.ledger='agent' AND t.deleted_at IS NULL)",
                [r.number], |r| r.get(0),
            )?;
            if !live { return Err(invalid()); }
            Ok(r)
        }).collect::<Result<Vec<_>>>()?;
        source_ids.sort_unstable_by_key(|r| r.number);
        source_ids.dedup_by_key(|r| r.number);
        let source_facts = source_ids
            .into_iter()
            .map(|reference| {
                let id = reference.id();
                let attributes = crate::todo_attributes::get(
                    c,
                    &Sender::Human,
                    &ChannelWorkObject {
                        kind: ChannelWorkKind::Requirement,
                        id: id.clone(),
                    },
                )?;
                if attributes.origin != TodoOrigin::Requirement {
                    return Err(invalid());
                }
                Ok((id, attributes.revision))
            })
            .collect::<Result<Vec<_>>>()?;
        let source_ids = source_facts.iter().map(|(id, _)| id.clone()).collect();
        let duplicate_id = format!("W{duplicate}");
        let survivor_id = format!("W{survivor}");
        // Two indexed point reads fence changed links even when their original
        // channel and event history have since been purged.
        let link_revisions = (
            crate::channel_work::work_link_revision(c, &duplicate_id)?,
            crate::channel_work::work_link_revision(c, &survivor_id)?,
        );
        let witness = judgments::fingerprint(&(
            &self.epoch,
            self.crew,
            duplicate,
            a.id,
            a.revision,
            survivor,
            b.id,
            b.revision,
            definition,
            source_facts,
            link_revisions,
        ))?;
        Ok(WorkDuplicateCandidate {
            duplicate_id,
            duplicate_revision: a.revision.to_string(),
            survivor_id,
            survivor_revision: b.revision.to_string(),
            source_ids,
            witness,
        })
    }

    /// The first matching live plan is the stable survivor. Later plans with
    /// work history remain separate and never become closure candidates.
    fn visit_candidates(
        &self,
        mut visit: impl FnMut(i64, i64, &str) -> Result<bool>,
    ) -> Result<()> {
        let mut firsts = BTreeMap::new();
        for (&number, &core) in &self.issued {
            let item = self.book.item(core).ok_or_else(invalid)?;
            let Some(definition) = self.definition(number, item)? else {
                continue;
            };
            match firsts.entry(definition) {
                std::collections::btree_map::Entry::Vacant(row) => {
                    row.insert(number);
                }
                std::collections::btree_map::Entry::Occupied(row) => {
                    if self.discardable(item) && !visit(number, *row.get(), row.key())? {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn list(c: &Connection, p: &WorkDuplicatesParams) -> Result<WorkDuplicatesPage> {
    crews::refuse_archived(c, &p.crew_id)?;
    let limit = usize::from(p.limit.unwrap_or(20));
    if !(1..=50).contains(&limit) {
        return Err(resource_acl::invalid("limit"));
    }
    let book = asks::load_org(c)?;
    let index = Index::load(c, &p.crew_id, &book)?;
    let scope = judgments::fingerprint(&(&index.epoch, &p.crew_id))?;
    let after = match p.after.as_deref() {
        None => 0,
        Some(raw) => {
            let (epoch, sequence) = raw
                .split_once(':')
                .ok_or_else(|| resource_acl::invalid("after"))?;
            let number = sequence
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0 && n.to_string() == sequence)
                .ok_or_else(|| resource_acl::invalid("after"))?;
            if epoch != scope {
                return Err(resource_acl::invalid("after"));
            }
            number
        }
    };
    let mut page = WorkDuplicatesPage {
        crew_id: p.crew_id.clone(),
        items: Vec::new(),
        next_after: None,
    };
    let mut last = 0;
    index.visit_candidates(|duplicate, survivor, definition| {
        if duplicate <= after {
            return Ok(true);
        }
        if page.items.len() == limit {
            page.next_after = Some(format!("{scope}:{last}"));
            return Ok(false);
        }
        page.items
            .push(index.make_candidate(c, duplicate, survivor, definition)?);
        last = duplicate;
        Ok(true)
    })?;
    Ok(page)
}

/// Called only by the authenticated Human command port. No discovery result
/// can authorize the existing factual close without fresh confirmation.
pub fn close(tx: &mut Tx<'_>, p: WorkDuplicateCloseParams) -> Result<WorkDuplicateClosure> {
    let duplicate = work::number(&p.duplicate_id)?;
    let survivor = work::number(&p.survivor_id)?;
    if !positive_revision(&p.expected_duplicate_revision)
        || !positive_revision(&p.expected_survivor_revision)
    {
        return Err(resource_acl::invalid("expectedRevision"));
    }
    if !digest(&p.witness) {
        return Err(resource_acl::invalid("witness"));
    }
    if !valid_text(&p.evidence.value) {
        return Err(resource_acl::invalid("evidence"));
    }
    if !valid_text(&p.note) {
        return Err(resource_acl::invalid("note"));
    }
    crews::refuse_archived(tx.conn(), &p.crew_id)?;
    let book = asks::load_org(tx.conn())?;
    let index = Index::load(tx.conn(), &p.crew_id, &book)?;
    let mut exact = None;
    index.visit_candidates(|number, kept, definition| {
        if number < duplicate {
            return Ok(true);
        }
        if number == duplicate && kept == survivor {
            exact = Some(index.make_candidate(tx.conn(), number, kept, definition)?);
        }
        Ok(false)
    })?;
    let current = exact
        .filter(|v| {
            v.witness == p.witness
                && v.duplicate_revision == p.expected_duplicate_revision
                && v.survivor_revision == p.expected_survivor_revision
        })
        .ok_or_else(changed)?;
    let core = index.issued[&duplicate];
    let seq = book.state().last_seq.checked_add(1).ok_or_else(invalid)?;
    let survivor_before = work::get(tx.conn(), &p.survivor_id)?;
    let closed = work::close_factual(
        tx,
        &Sender::Human,
        WorkCloseParams {
            id: p.duplicate_id.clone(),
            expected_revision: p.expected_duplicate_revision.clone(),
            kind: WorkClosureKind::Duplicate,
            evidence: p.evidence.clone(),
            note: p.note.clone(),
        },
    )?;
    let seq_sql = i64::try_from(seq).map_err(|_| invalid())?;
    let raw: String = tx.conn().query_row(
        "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
        params![seq_sql, p.crew_id],
        |r| r.get(0),
    )?;
    let event: Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
    let receipt = WorkDuplicateClosure {
        crew_id: p.crew_id,
        duplicate_id: p.duplicate_id,
        previous_duplicate_revision: p.expected_duplicate_revision,
        duplicate_revision: closed.revision,
        survivor_id: p.survivor_id,
        survivor_revision: current.survivor_revision,
        event_seq: seq.to_string(),
        closed_at_ms: event.at.0,
        evidence: p.evidence,
        note: p.note,
    };
    if event.seq != seq
        || !matches_event(&receipt, &event, core)
        || closed.status != "dropped"
        || !exact_next(
            &receipt.previous_duplicate_revision,
            &receipt.duplicate_revision,
        )
        || work::get(tx.conn(), &receipt.survivor_id)? != survivor_before
        || asks::load_org(tx.conn())?.state().last_seq != seq
    {
        return Err(invalid());
    }
    let stored = Stored {
        version: 1,
        checksum: judgments::fingerprint(&receipt)?,
        closure: receipt,
    };
    let key = history_key(seq);
    if meta_get(tx.conn(), &key)?.is_some() {
        return Err(invalid());
    }
    meta_set(
        tx.conn(),
        &key,
        &serde_json::to_string(&stored).map_err(StoreError::from)?,
    )?;
    Ok(stored.closure)
}

fn history_key(seq: u64) -> String {
    format!("work.duplicate.review.v1:{seq:020}")
}

fn matches_event(receipt: &WorkDuplicateClosure, event: &Event, core: WorkId) -> bool {
    let WorkEvidenceParam { kind, value } = &receipt.evidence;
    let proof = match kind {
        WorkEvidenceKind::Commit => Evidence::Commit(value.clone()),
        WorkEvidenceKind::Url => Evidence::Url(value.clone()),
        WorkEvidenceKind::Text => Evidence::Text(value.clone()),
    };
    event.by == Actor::Human
        && event.at.0 == receipt.closed_at_ms
        && event.seq.to_string() == receipt.event_seq
        && matches!(&event.kind, EventKind::Dropped { id, reason } if *id == core
            && reason.kind == DropKind::Duplicate && reason.note == receipt.note
            && reason.evidence.as_ref() == Some(&proof))
}

pub(crate) fn history(
    c: &Connection,
    crew: &str,
    id: &str,
    core: WorkId,
    event: &Event,
) -> Result<Option<WorkDuplicateClosure>> {
    let Some(raw) = meta_get(c, &history_key(event.seq))? else {
        return Ok(None);
    };
    let saved: Stored = serde_json::from_str(&raw).map_err(StoreError::from)?;
    let r = &saved.closure;
    if saved.version != 1
        || r.crew_id != crew
        || r.duplicate_id != id
        || !exact_next(&r.previous_duplicate_revision, &r.duplicate_revision)
        || !positive_revision(&r.survivor_revision)
        || work::number(&r.survivor_id)? >= work::number(id)?
        || !valid_text(&r.evidence.value)
        || !valid_text(&r.note)
        || saved.checksum != judgments::fingerprint(r)?
        || !matches_event(r, event, core)
    {
        return Err(invalid());
    }
    Ok(Some(saved.closure))
}

pub fn reviewed_history(
    c: &Connection,
    p: WorkDuplicateHistoryParams,
) -> Result<WorkClosureHistoryPage> {
    crate::work_close::history_scoped(
        c,
        WorkClosureHistoryParams {
            id: p.id,
            before_seq: p.before_seq,
            limit: p.limit,
        },
        Some(&p.crew_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        channel_work, requirements,
        testkit::{NOW, ok, store},
    };
    use crewd_proto::{
        ChannelWorkKind, ChannelWorkLinkParams, ChannelWorkObject, TodoCreateParams,
        WorkCreateParams, WorkLevel, WorkPlan,
    };
    use crewd_store::Store;

    fn original(s: &mut Store) -> String {
        ok(s, |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: None,
                    text: "original fixture request".into(),
                },
            )
        })
        .todo
        .requirement_id
        .unwrap()
    }

    fn plan(s: &mut Store, source: &str, owner: Option<&str>, level: WorkLevel) -> WorkPlan {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "copy prepared notes".into(),
                    goal: "collect current notes".into(),
                    acceptance: "checked against source".into(),
                    sources: vec![source.into()],
                    owner: owner.map(str::to_owned),
                    level,
                    ..Default::default()
                },
            )
        })
    }

    fn list_params() -> WorkDuplicatesParams {
        WorkDuplicatesParams {
            crew_id: "crew-a".into(),
            after: None,
            limit: None,
        }
    }

    fn close_params(candidate: WorkDuplicateCandidate) -> WorkDuplicateCloseParams {
        WorkDuplicateCloseParams {
            crew_id: "crew-a".into(),
            duplicate_id: candidate.duplicate_id,
            expected_duplicate_revision: candidate.duplicate_revision,
            survivor_id: candidate.survivor_id,
            expected_survivor_revision: candidate.survivor_revision,
            witness: candidate.witness,
            evidence: WorkEvidenceParam {
                kind: WorkEvidenceKind::Text,
                value: "verified same task in original records".into(),
            },
            note: "keep the earlier plan and its real assignment".into(),
        }
    }

    #[test]
    fn exact_duplicate_closure_preserves_survivor_and_exposes_only_scoped_typed_history() {
        let mut s = store();
        let source = original(&mut s);
        let kept = plan(&mut s, &source, Some("w-a1"), WorkLevel::Local);
        let duplicate = plan(&mut s, &source, None, WorkLevel::Local);
        let different = plan(&mut s, &source, None, WorkLevel::Public);
        let before = s.conn().total_changes();
        let [candidate] = list(s.conn(), &list_params())
            .unwrap()
            .items
            .try_into()
            .unwrap();
        assert_eq!(s.conn().total_changes(), before);
        assert_eq!(candidate.source_ids, vec![source.clone()]);
        assert_eq!(candidate.survivor_id, kept.id);
        assert_eq!(candidate.duplicate_id, duplicate.id);
        let action = close_params(candidate);
        s.conn().execute_batch(
            "CREATE TRIGGER fixture_receipt_abort BEFORE INSERT ON meta
             WHEN NEW.key LIKE 'work.duplicate.review.v1:%' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;"
        ).unwrap();
        assert!(crate::run(&mut s, NOW, |tx| close(tx, action.clone())).is_err());
        assert_eq!(work::get(s.conn(), &duplicate.id).unwrap(), duplicate);
        s.conn()
            .execute_batch("DROP TRIGGER fixture_receipt_abort")
            .unwrap();
        let settled = ok(&mut s, |tx| close(tx, action.clone()));
        assert!(crate::run(&mut s, NOW, |tx| close(tx, action)).is_err());
        assert_eq!(work::get(s.conn(), &kept.id).unwrap(), kept);
        assert_eq!(work::get(s.conn(), &different.id).unwrap(), different);
        assert_eq!(
            work::get(s.conn(), &duplicate.id).unwrap().status,
            "dropped"
        );
        assert_eq!(
            requirements::get(s.conn(), &source).unwrap().todo.text,
            "original fixture request"
        );
        assert!(list(s.conn(), &list_params()).unwrap().items.is_empty());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        let params = WorkDuplicateHistoryParams {
            crew_id: "crew-a".into(),
            id: duplicate.id.clone(),
            before_seq: None,
            limit: Some(1),
        };
        let page = reviewed_history(s.conn(), params.clone()).unwrap();
        assert_eq!(page.items[0].duplicate_review.as_ref(), Some(&settled));
        assert_eq!(page.items[0].seq, settled.event_seq);
        let ordinary = crate::work_close::history(
            s.conn(),
            WorkClosureHistoryParams {
                id: duplicate.id,
                before_seq: None,
                limit: Some(1),
            },
        )
        .unwrap();
        assert!(ordinary.items[0].duplicate_review.is_none());
        let foreign = WorkDuplicateHistoryParams {
            crew_id: "crew-b".into(),
            ..params.clone()
        };
        assert!(reviewed_history(s.conn(), foreign).is_err());
        meta_set(
            s.conn(),
            &history_key(settled.event_seq.parse().unwrap()),
            "damaged",
        )
        .unwrap();
        assert!(reviewed_history(s.conn(), params).is_err());
    }

    #[test]
    fn duplicate_candidate_is_fenced_by_scope_history_live_references_and_restore() {
        let mut s = store();
        let source = original(&mut s);
        let kept = plan(&mut s, &source, Some("w-a1"), WorkLevel::Local);
        let duplicate = plan(&mut s, &source, None, WorkLevel::Local);
        let next = plan(&mut s, &source, None, WorkLevel::Local);
        let mut p = list_params();
        p.limit = Some(1);
        let first = list(s.conn(), &p).unwrap();
        let action = close_params(first.items[0].clone());
        let object = ChannelWorkObject {
            kind: ChannelWorkKind::Work,
            id: duplicate.id.clone(),
        };
        for linked in [true, false] {
            ok(&mut s, |tx| {
                let channel = "crew-a/lobby";
                let rev = crewd_store::channel_work::revision(tx.conn(), channel)?;
                channel_work::link(
                    tx,
                    &Sender::Human,
                    ChannelWorkLinkParams {
                        channel_id: channel.into(),
                        object: object.clone(),
                        expected_link_revision: rev.to_string(),
                    },
                    linked,
                )
            });
        }
        let current = list(s.conn(), &p).unwrap();
        assert_ne!(current.items[0].witness, action.witness);
        assert!(crate::run(&mut s, NOW, |tx| close(tx, action)).is_err());
        let before_purge = close_params(current.items[0].clone());
        ok(&mut s, |tx| {
            let channel = "crew-x/lobby";
            let rev = crewd_store::channel_work::revision(tx.conn(), channel)?;
            channel_work::link(
                tx,
                &Sender::Human,
                ChannelWorkLinkParams {
                    channel_id: channel.into(),
                    object,
                    expected_link_revision: rev.to_string(),
                },
                true,
            )
        });
        assert_eq!(list(s.conn(), &p).unwrap().items[0].duplicate_id, next.id);
        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crews::purge(tx, "crew-x"));
        let after_purge = list(s.conn(), &p).unwrap();
        assert_eq!(after_purge.items[0].duplicate_id, duplicate.id);
        assert_ne!(after_purge.items[0].witness, before_purge.witness);
        assert!(crate::run(&mut s, NOW, |tx| close(tx, before_purge)).is_err());
        let before_reference = close_params(after_purge.items[0].clone());
        ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-b".into(),
                    title: "foreign live dependency".into(),
                    depends_on: vec![duplicate.id.clone()],
                    ..Default::default()
                },
            )
        });
        assert!(crate::run(&mut s, NOW, |tx| close(tx, before_reference)).is_err());
        let remaining = list(s.conn(), &p).unwrap();
        assert_eq!(remaining.items[0].duplicate_id, next.id);
        assert_eq!(remaining.items[0].survivor_id, kept.id);
        let old_action = close_params(remaining.items[0].clone());
        p.after = first.next_after;
        assert!(p.after.is_some());
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(list(s.conn(), &p).is_err());
        assert!(crate::run(&mut s, NOW, |tx| close(tx, old_action)).is_err());
        assert_eq!(work::get(s.conn(), &duplicate.id).unwrap().status, "open");
        assert_eq!(work::get(s.conn(), &next.id).unwrap().status, "open");
    }

    #[test]
    fn manual_requirement_change_invalidates_duplicate_confirmation_after_reopen() {
        let mut s = store();
        let source = original(&mut s);
        plan(&mut s, &source, Some("w-a1"), WorkLevel::Local);
        let duplicate = plan(&mut s, &source, None, WorkLevel::Local);
        let object = ChannelWorkObject {
            kind: ChannelWorkKind::Requirement,
            id: source,
        };
        ok(&mut s, |tx| {
            let channel = "crew-a/lobby";
            let rev = crewd_store::channel_work::revision(tx.conn(), channel)?;
            channel_work::link(
                tx,
                &Sender::Human,
                ChannelWorkLinkParams {
                    channel_id: channel.into(),
                    object: object.clone(),
                    expected_link_revision: rev.to_string(),
                },
                true,
            )
        });
        let before = list(s.conn(), &list_params()).unwrap().items.remove(0);
        for state in [
            crewd_proto::ManualTodoState::Done,
            crewd_proto::ManualTodoState::Open,
        ] {
            ok(&mut s, |tx| {
                let current = crate::todo_attributes::get(tx.conn(), &Sender::Human, &object)?;
                crate::todo_attributes::set_state(
                    tx,
                    &Sender::Human,
                    crewd_proto::TodoSetStateParams {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        object: object.clone(),
                        channel_id: "crew-a/lobby".into(),
                        expected_revision: current.revision,
                        state,
                    },
                )
            });
        }
        let after = list(s.conn(), &list_params()).unwrap().items.remove(0);
        assert_eq!(after.duplicate_id, duplicate.id);
        assert_ne!(after.witness, before.witness);
        assert!(crate::run(&mut s, NOW, |tx| close(tx, close_params(before))).is_err());
        assert_eq!(work::get(s.conn(), &duplicate.id).unwrap().status, "open");
    }
}
