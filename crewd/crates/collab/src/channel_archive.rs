//! Explicit topic lifecycle checks. Facts are not an inactivity judgment or an Admin grant.
use crate::{Result, Sender, Tx, asks, channels, crews, judgments, wakes, work};
use crewd_proto::{ChannelKind, ChannelWorkKind, ChannelWorkObject, channel_archive::*};
use crewd_store::{StoreError, object_refs::ObjectKind};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
const MAX: usize = 200;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lifecycle {
    version: u32,
    revision: String,
    last_action: Option<ChannelArchiveAction>,
}
fn key(ch: &str) -> String {
    format!("channel.archive.v1:{ch}")
}
fn invalid(field: &str) -> crate::CollabError {
    crate::resource_acl::invalid(field)
}
fn lifecycle(c: &Connection, ch: &str) -> Result<Option<Lifecycle>> {
    let s: Option<Lifecycle> = crewd_store::meta_get(c, &key(ch))?
        .map(|v| serde_json::from_str(&v).map_err(StoreError::from))
        .transpose()?;
    if s.as_ref()
        .is_some_and(|s| s.version != 1 || uuid::Uuid::parse_str(&s.revision).is_err())
    {
        return Err(invalid("archive metadata"));
    }
    Ok(s)
}
/// All legacy manual archive/restore requests also advance this witness, including
/// same-value writes, so archive→restore cannot revive a previously issued token.
pub(crate) fn note_lifecycle(tx: &mut Tx<'_>, ch: &str) -> Result<()> {
    // Legacy manual management never parses or erases optional action metadata.
    crewd_store::meta_set(
        tx.conn(),
        &format!("channel.archive.generation.v1:{ch}"),
        &uuid::Uuid::new_v4().to_string(),
    )?;
    Ok(())
}

pub(crate) fn generation(c: &Connection, ch: &str) -> Result<Option<String>> {
    Ok(crewd_store::meta_get(c, &format!("channel.archive.generation.v1:{ch}"))?)
}

fn authorize_read(c: &Connection, by: &Sender, ch: &str) -> Result<()> {
    crate::channel_work::authorize(c, by, ch, false)?;
    Ok(())
}
fn rows(c: &Connection, sql: &str, ch: &str) -> Result<Vec<serde_json::Value>> {
    let mut q = c.prepare(sql)?;
    Ok(q.query_map([ch], |r| {
        Ok(serde_json::json!([
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?
        ]))
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn check(
    c: &Connection,
    by: &Sender,
    p: ChannelArchiveCheckParams,
    now_ms: i64,
) -> Result<ChannelArchiveCheck> {
    let ch = channels::get(c, &p.channel_id)?;
    authorize_read(c, by, &ch.id)?;
    let permitted = channels::may_manage(c, by, &ch)?;
    let crew_archived = crews::is_archived(c, &ch.crew_id)?;
    let lifecycle = lifecycle(c, &ch.id)?;
    let generation = generation(c, &ch.id)?;
    let b = asks::load_org(c)?;
    let mut facts = ChannelArchiveFacts::default();
    let mut observed = Vec::<serde_json::Value>::new();
    let mut works = BTreeMap::new();
    let mut requirements = BTreeSet::new();
    let mut related_asks = BTreeSet::new();
    // Legacy channel pointers and explicit modern associations share one source authority.
    for (id, w) in &b.state().items {
        if w.channel.as_ref().is_some_and(|s| s.as_str() == ch.id) {
            if works.len() >= MAX {
                facts.incomplete = true;
                break;
            }
            let crew = b
                .state()
                .item_crew(*id)
                .ok_or_else(|| invalid("work crew"))?;
            let public = work::reference(c, ObjectKind::Plan, crew.as_str(), id)?;
            works.insert(*id, public);
        }
    }
    let mut q=c.prepare("SELECT object_kind,object_id,source_crew_id FROM channel_work_links WHERE channel_id=?1 ORDER BY object_kind,object_id LIMIT 201")?;
    let links = q
        .query_map([&ch.id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    facts.incomplete |= links.len() > MAX;
    for (kind, id, source_crew) in links.into_iter().take(MAX) {
        let (kind, object_kind) = match kind.as_str() {
            "work" => (ChannelWorkKind::Work, ObjectKind::Plan),
            "requirement" => (ChannelWorkKind::Requirement, ObjectKind::Requirement),
            "humanTask" => (ChannelWorkKind::HumanTask, ObjectKind::HumanTask),
            _ => return Err(invalid("linked object kind")),
        };
        let s = crate::channel_work::summary(
            c,
            &b,
            ChannelWorkObject {
                kind: kind.clone(),
                id: id.clone(),
            },
        )?;
        if s.crew_id != source_crew {
            return Err(invalid("linked source crew"));
        }
        if crews::get(c, &source_crew)?.removed_at.is_some() {
            facts.incomplete = true;
        }
        observed.push(serde_json::to_value(&s).map_err(StoreError::from)?);
        let number = id
            .get(1..)
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| invalid("linked id"))?;
        let core = crewd_store::work_organization::core_for_reference(c, object_kind, number)?
            .ok_or_else(|| invalid("linked core identity"))?;
        match kind {
            ChannelWorkKind::Work => {
                works.insert(core.parse().map_err(|_| invalid("work id"))?, id);
            }
            ChannelWorkKind::Requirement => {
                let entry_no = core.parse().map_err(|_| invalid("entry id"))?;
                let entry = b
                    .entry_view(entry_no)
                    .ok_or_else(|| invalid("missing entry"))?;
                // A linked requirement also attributes its actual work, including
                // dropped work whose notification may still be in flight.
                for (work_id, _) in b
                    .state()
                    .items
                    .iter()
                    .filter(|(_, w)| w.sources.contains(&entry_no))
                {
                    if !works.contains_key(work_id) && works.len() > MAX {
                        facts.incomplete = true;
                        break;
                    }
                    let crew = b
                        .state()
                        .item_crew(*work_id)
                        .ok_or_else(|| invalid("entry work crew"))?;
                    works.insert(
                        *work_id,
                        work::reference(c, ObjectKind::Plan, crew.as_str(), work_id)?,
                    );
                }
                observed.push(serde_json::json!({"entry":id,"derived":entry}));
                requirements.insert(id);
                facts.linked_requirements += 1;
                // A manual checklist completion is not permission to hide still-live
                // work attached to the immutable original requirement.
                if entry.status == crewd_work::EntryStatus::InProgress
                    || !matches!(s.status.as_str(), "done" | "closed")
                {
                    facts.open_requirements += 1;
                }
            }
            ChannelWorkKind::HumanTask => {
                related_asks.insert(core.parse().map_err(|_| invalid("ask id"))?);
            }
        }
    }
    if works.len() > MAX {
        facts.incomplete = true;
    }
    for (id, public) in works.iter().take(MAX) {
        let w = b.item(*id).ok_or_else(|| invalid("missing work"))?;
        facts.linked_work += 1;
        facts.live_work += u64::from(w.status.is_live());
        let crew = b
            .state()
            .item_crew(*id)
            .ok_or_else(|| invalid("work crew"))?;
        if crews::get(c, crew.as_str())?.removed_at.is_some() {
            facts.incomplete = true;
        }
        observed.push(serde_json::json!({"work":public,"state":w}));
    }
    // Direct association, work-linked questions (including shared blockers), and actual
    // announcements in this channel all count. An unanswered unrelated crew ask does not.
    for (id, a) in &b.state().asks {
        if a.item.is_some_and(|w| works.contains_key(&w))
            || works.keys().any(|w| {
                b.item(*w)
                    .is_some_and(|w| w.blocked_on.contains(&crewd_work::Blocker::Human(*id)))
            })
        {
            related_asks.insert(*id);
        }
    }
    let mut q=c.prepare("SELECT m.id FROM messages m JOIN ask_meta a ON a.message_id=m.id WHERE m.channel_id=?1 ORDER BY m.seq DESC LIMIT 201")?;
    let announced = q
        .query_map([&ch.id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    facts.incomplete |= announced.len() > MAX;
    for mid in announced.into_iter().take(MAX) {
        if let Some((_, id)) = asks::by_message(c, &mid)? {
            let n = id[1..].parse::<i64>().map_err(|_| invalid("ask id"))?;
            let core =
                crewd_store::work_organization::core_for_reference(c, ObjectKind::HumanTask, n)?
                    .ok_or_else(|| invalid("ask core identity"))?;
            related_asks.insert(core.parse().map_err(|_| invalid("ask id"))?);
        }
    }
    if related_asks.len() > MAX {
        facts.incomplete = true;
    }
    let mut ask_sources = Vec::new();
    for id in related_asks.iter().take(MAX) {
        let a = b.ask(*id).ok_or_else(|| invalid("missing ask"))?;
        let crew = b.state().ask_crew(*id).ok_or_else(|| invalid("ask crew"))?;
        let public = work::reference(c, ObjectKind::HumanTask, crew.as_str(), id)?;
        ask_sources.push(wakes::source::ask(crew.as_str(), &public));
        let open = if a.human_task {
            let attributes = crate::todo_attributes::get(
                c,
                &Sender::Human,
                &ChannelWorkObject {
                    kind: ChannelWorkKind::HumanTask,
                    id: public.clone(),
                },
            )?;
            let open = attributes.manual_state != Some(crewd_proto::ManualTodoState::Done);
            observed.push(serde_json::to_value(attributes).map_err(StoreError::from)?);
            open
        } else {
            matches!(a.state, crewd_work::AskState::Open)
        };
        facts.open_asks += u64::from(open);
        observed.push(serde_json::json!({"ask":public,"state":a}));
    }
    let last: Option<(i64, String)> = c
        .query_row(
            "SELECT seq,created_at FROM messages WHERE channel_id=?1 ORDER BY seq DESC LIMIT 1",
            [&ch.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((seq, at)) = last {
        facts.last_message_seq = Some(seq.to_string());
        facts.last_message_at = Some(at);
    }
    let member_rows = rows(
        c,
        "SELECT member_kind,member_id,joined_at FROM channel_members WHERE channel_id=?1 AND left_at IS NULL ORDER BY member_kind,member_id LIMIT 1025",
        &ch.id,
    )?;
    let listen_rows = rows(
        c,
        "SELECT session_id,CAST(until AS TEXT),senders FROM listens WHERE channel_id=?1 ORDER BY session_id LIMIT 1025",
        &ch.id,
    )?;
    facts.incomplete |= member_rows.len() > 1024 || listen_rows.len() > 1024;
    let (reservations, reservations_incomplete) =
        crate::cursor_reservations::archive_witness(c, &ch.id)?;
    facts.active_deliveries = reservations.len() as u64;
    facts.incomplete |= reservations_incomplete;
    // Only current channel pending model facts are consulted, never a global 256-row sample.
    let pending = unsettled_judgments(c, &ch.id)?;
    facts.incomplete |= pending.len() > 256;
    for id in pending.into_iter().take(256) {
        if let Some(r) =
            judgments::record(c, &crewd_proto::judgments::JudgmentSourceKind::Message, &id)?
        {
            if matches!(
                r.phase,
                crewd_proto::judgments::JudgmentPhase::Pending
                    | crewd_proto::judgments::JudgmentPhase::Dispatched
            ) {
                facts.pending_judgments += 1;
            }
            observed.push(serde_json::to_value(r).map_err(StoreError::from)?);
        }
    }
    let work_ids: Vec<String> = works.values().take(MAX).cloned().collect();
    notification_facts(
        c,
        NotificationScope {
            channel: &ch.id,
            work_ids: &work_ids,
            requirements: &requirements,
            ask_sources: &ask_sources,
        },
        now_ms,
        &mut facts,
        &mut observed,
    )?;
    let mut blockers = Vec::new();
    use ChannelArchiveBlocker::*;
    for (yes, reason) in [
        (facts.linked_work == 0, NoLinkedWork),
        (ch.kind != ChannelKind::Topic, NotTopic),
        (crew_archived, CrewArchived),
        (!permitted, NotAuthorized),
        (ch.archived_at.is_some(), AlreadyArchived),
        (facts.live_work > 0, LiveWork),
        (facts.open_requirements > 0, OpenRequirement),
        (facts.open_asks > 0, OpenAsk),
        (facts.pending_wakes > 0, PendingWake),
        (facts.inflight_wakes > 0, InflightWake),
        (facts.active_deliveries > 0, ActiveDelivery),
        (facts.pending_judgments > 0, PendingJudgment),
        (ch.pending_owner.is_some(), PendingOwnerTransfer),
        (facts.incomplete, Incomplete),
    ] {
        if yes {
            blockers.push(reason);
        }
    }
    let revision = judgments::fingerprint(
        &serde_json::json!({"channel":ch,"facts":facts,"objects":observed,"members":member_rows,"listens":listen_rows,"reservations":reservations,"links":crewd_store::channel_work::revision(c,&ch.id)?,"lifecycle":lifecycle,"generation":generation,"db":crewd_store::meta_get(c,"dbId")?,"epoch":crewd_store::judgment_state::epoch(c)?}),
    )?;
    Ok(ChannelArchiveCheck {
        can_archive: blockers.is_empty(),
        can_restore: ch.kind == ChannelKind::Topic
            && ch.archived_at.is_some()
            && !crew_archived
            && permitted,
        channel: ch,
        revision,
        blockers,
        facts,
        last_action: lifecycle
            .filter(|s| generation.as_deref() == Some(s.revision.as_str()))
            .and_then(|s| s.last_action),
    })
}

/// Skip only structurally valid, explicitly terminal model records. All other
/// records pass through the canonical typed reader. This avoids both treating
/// unknown states as settled and imposing a cap on healthy historical messages.
fn unsettled_judgments(c: &Connection, channel: &str) -> Result<Vec<String>> {
    let mut safe=vec![
  "json_type(j)='object'".to_string(),
  "json_extract(j,'$.version')=1".into(),
  "json_extract(j,'$.phase') IN ('settled','suppressed','historical')".into(),
  "json_type(j,'$.preparedAtMs')='integer'".into(),
  "length(json_extract(j,'$.decisionId'))>0".into(),
  "length(json_extract(j,'$.dbId'))>0".into(),
  "json_type(j,'$.diagnostic') IS NOT NULL".into(),
  "json_type(j,'$.recipients')='array'".into(),
  "NOT EXISTS(SELECT 1 FROM json_each(j,'$.recipients') WHERE type!='text')".into(),
  "json_type(j,'$.capturedFloors')='object'".into(),
  "NOT EXISTS(SELECT 1 FROM json_each(j,'$.capturedFloors') WHERE type!='integer' OR atom<0)".into(),
  "json_type(j,'$.capturedStopEpochs')='object'".into(),
  "NOT EXISTS(SELECT 1 FROM json_each(j,'$.capturedStopEpochs') WHERE type!='text')".into(),
  "(json_type(j,'$.attribution') IS NULL OR json_type(j,'$.attribution')='null' OR (json_type(j,'$.attribution')='object' AND json_extract(j,'$.attribution.kind') IN ('message','work','requirement') AND json_type(j,'$.attribution.id')='text' AND json_type(j,'$.attribution.revision')='text'))".into(),
  "NOT EXISTS(SELECT 1 FROM json_tree(j) WHERE key IS NOT NULL GROUP BY parent,key HAVING count(*)>1)".into(),
 ];
    for key in [
        "decisionId",
        "dbId",
        "restoreEpoch",
        "configRevision",
        "sourceVersion",
    ] {
        safe.push(format!("json_type(j,'$.{key}')='text'"));
    }
    // CASE makes malformed outer JSON safe before any JSON function evaluates it.
    let sql = format!(
        "WITH records AS (SELECT id,seq,CASE WHEN NOT json_valid(extra) THEN '{{}}' WHEN (SELECT count(*) FROM json_each(extra) WHERE key='judgmentV1')>1 THEN '{{}}' ELSE json_extract(extra,'$.judgmentV1') END AS j FROM messages WHERE channel_id=?1 AND extra IS NOT NULL AND CASE WHEN json_valid(extra) THEN json_type(extra,'$.judgmentV1') IS NOT NULL ELSE 1 END) SELECT id FROM records WHERE NOT coalesce(({}),0) ORDER BY seq LIMIT 257",
        safe.join(" AND ")
    );
    let mut q = c.prepare(&sql)?;
    Ok(q.query_map([channel], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
struct NotificationScope<'a> {
    channel: &'a str,
    work_ids: &'a [String],
    requirements: &'a BTreeSet<String>,
    ask_sources: &'a [String],
}
fn notification_facts(
    c: &Connection,
    scope: NotificationScope<'_>,
    now: i64,
    facts: &mut ChannelArchiveFacts,
    observed: &mut Vec<serde_json::Value>,
) -> Result<()> {
    let NotificationScope {
        channel,
        work_ids,
        requirements,
        ask_sources,
    } = scope;
    let mut durable = Vec::new();
    for id in work_ids {
        durable.extend(crate::work_coordination::archive_wake_ids(c, id)?);
        durable.extend(crate::work_reassignment::archive_wake_ids(c, id)?);
    }
    let mut subjects = work_ids.to_vec();
    subjects.extend(requirements.iter().cloned());
    let mut exact = ask_sources.to_vec();
    for id in requirements {
        exact.push(format!("requirement-new:{id}"));
        exact.push(format!("judgment-route:{id}"));
    }
    let mut prefixes = Vec::new();
    for id in work_ids {
        for kind in [
            "work-lost",
            "work-transfer",
            "work-assigned",
            "work-close",
            "work-done",
        ] {
            prefixes.push(format!("{kind}:{id}:"));
        }
    }
    // Select only candidate notifications attributable to this channel. String matches
    // only narrow the query: the original typed renderer validates each work fact.
    // Durable fact/wake identities cover followups whose notes contain opaque fact IDs.
    let sql="SELECT w.id,w.session_id,w.kind,w.source,w.origin,w.priority,w.not_before,w.note,w.state,w.attempts,w.last_error,w.created_at,w.settled_at FROM wakes w WHERE w.state IN ('pending','inflight') AND (
 (w.kind='message' AND EXISTS(SELECT 1 FROM messages m WHERE m.channel_id=?1 AND w.source='msg:'||m.id))
 OR w.source IN (SELECT value FROM json_each(?2))
 OR w.id IN (SELECT value FROM json_each(?3))
 OR EXISTS(SELECT 1 FROM work_followup_facts f WHERE f.original_wake_id=w.id AND 'W'||f.subject_number IN (SELECT value FROM json_each(?4)))
 OR EXISTS(SELECT 1 FROM json_tree(CASE WHEN json_valid(w.note) THEN w.note ELSE '{}' END) j WHERE j.type='text' AND j.atom IN(SELECT value FROM json_each(?4)))
 OR EXISTS(SELECT 1 FROM json_each(?5) prefix WHERE substr(w.source,1,length(prefix.value))=prefix.value)
 ) ORDER BY w.id LIMIT 513";
    let mut q = c.prepare(sql)?;
    let selected = q
        .query_map(
            params![
                channel,
                serde_json::to_string(&exact).map_err(StoreError::from)?,
                serde_json::to_string(&durable).map_err(StoreError::from)?,
                serde_json::to_string(&subjects).map_err(StoreError::from)?,
                serde_json::to_string(&prefixes).map_err(StoreError::from)?
            ],
            wakes::row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    facts.incomplete |= selected.len() > 512;
    for row in selected.into_iter().take(512) {
        observed.push(serde_json::json!({"wake":row.id,"kind":row.kind,"session":row.session,"source":row.source,"state":row.state,"attempts":row.attempts,"note":row.note}));
        // An in-flight write cannot be recalled by declaring its fact stale; wait for
        // the existing typed delivery acknowledgment before changing channel lifecycle.
        if row.state == wakes::WakeState::Inflight {
            facts.inflight_wakes += 1;
            continue;
        }
        let stale = match row.kind {
            wakes::WakeKind::Message => {
                !channels::channels_of_session(c, &row.session)?
                    .iter()
                    .any(|ch| ch.id == channel)
                    && !channels::has_listen_row(c, channel, &row.session, now)?
            }
            wakes::WakeKind::AskAnswer => false,
            _ => match work::followup_note(c, &row, now)? {
                work::FollowupNote::Stale => true,
                work::FollowupNote::Current(_) => false,
                work::FollowupNote::Ordinary => {
                    facts.incomplete = true;
                    false
                }
            },
        };
        if stale {
            facts.stale_wakes += 1;
        } else {
            facts.pending_wakes += 1;
        }
    }
    Ok(())
}

fn mutate(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: String,
    expected: String,
    note: String,
    evidence: Option<String>,
    archive: bool,
) -> Result<ChannelArchiveCheck> {
    if note.trim().is_empty()
        || note.len() > 8192
        || evidence
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.len() > 8192)
    {
        return Err(invalid("evidence and note"));
    }
    let before = check(
        tx.conn(),
        by,
        ChannelArchiveCheckParams {
            channel_id: id.clone(),
        },
        tx.now_ms(),
    )?;
    if before.revision != expected {
        return Err(crate::CollabError::Conflict(
            crate::refusal::Refusal::new(crate::refusal::Code::ResourceRequestConflict)
                .arg("request", "channel archive revision"),
        ));
    }
    if !(if archive {
        before.can_archive
    } else {
        before.can_restore
    }) {
        return Err(invalid("channel archive blocked"));
    }
    let by_name = match by {
        Sender::Human => "human".into(),
        Sender::Session(s) => s.clone(),
        Sender::System => return Err(invalid("actor")),
    };
    channels::update(
        tx,
        by,
        &id,
        channels::ChannelPatch {
            archived: Some(archive),
            ..Default::default()
        },
    )?;
    let mut state = Lifecycle {
        version: 1,
        revision: crewd_store::meta_get(tx.conn(), &format!("channel.archive.generation.v1:{id}"))?
            .ok_or_else(|| invalid("lifecycle generation"))?,
        last_action: None,
    };
    state.last_action = Some(ChannelArchiveAction {
        action: if archive {
            ChannelArchiveActionKind::Archive
        } else {
            ChannelArchiveActionKind::Restore
        },
        actor: by_name,
        at: tx.now_ts(),
        note: note.trim().into(),
        evidence: evidence.map(|s| s.trim().into()),
    });
    let audit = serde_json::json!({"version":1,"channelId":id,"beforeRevision":expected,"afterRevision":state.revision,"action":state.last_action,"epoch":crewd_store::judgment_state::epoch(tx.conn())?});
    crewd_store::meta_set(
        tx.conn(),
        &format!("channel.archive.audit.v1:{}:{}", id, state.revision),
        &audit.to_string(),
    )?;
    crewd_store::meta_set(
        tx.conn(),
        &key(&id),
        &serde_json::to_string(&state).map_err(StoreError::from)?,
    )?;
    check(
        tx.conn(),
        by,
        ChannelArchiveCheckParams { channel_id: id },
        tx.now_ms(),
    )
}
pub fn archive(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: ChannelArchiveVerifiedParams,
) -> Result<ChannelArchiveCheck> {
    mutate(
        tx,
        by,
        p.channel_id,
        p.expected_revision,
        p.note,
        Some(p.evidence),
        true,
    )
}
pub fn restore(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: ChannelRestoreVerifiedParams,
) -> Result<ChannelArchiveCheck> {
    mutate(
        tx,
        by,
        p.channel_id,
        p.expected_revision,
        p.note,
        None,
        false,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        channels,
        testkit::{ok, store},
    };
    use crewd_proto::ChannelKind;
    #[test]
    fn explicit_topic_check_is_required_before_safe_archive() {
        let mut s = store();
        let ch = ok(&mut s, |tx| {
            channels::create(
                tx,
                &Sender::Human,
                channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "reviewed topic".into(),
                    ..Default::default()
                },
            )
        });
        let check = check(
            s.conn(),
            &Sender::Human,
            ChannelArchiveCheckParams { channel_id: ch.id },
            crate::testkit::NOW,
        )
        .unwrap();
        assert!(!check.can_archive);
        assert!(
            check
                .blockers
                .contains(&ChannelArchiveBlocker::NoLinkedWork)
        );
    }
    fn ready(s: &mut crewd_store::Store) -> String {
        let ch = ok(s, |tx| {
            channels::create(
                tx,
                &Sender::Human,
                channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "verified archive".into(),
                    ..Default::default()
                },
            )
        });
        let w = ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "finished".into(),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    channel_id: Some(ch.id.clone()),
                    ..Default::default()
                },
            )
        });
        ok(s, |tx| {
            work::close_factual(
                tx,
                &Sender::Human,
                crewd_proto::WorkCloseParams {
                    id: w.id,
                    expected_revision: w.revision,
                    kind: crewd_proto::WorkClosureKind::Done,
                    note: "verified".into(),
                    evidence: crewd_proto::WorkEvidenceParam {
                        kind: crewd_proto::WorkEvidenceKind::Text,
                        value: "checked result".into(),
                    },
                },
            )
        });
        ch.id
    }
    fn inspect(s: &crewd_store::Store, id: &str) -> Result<ChannelArchiveCheck> {
        check(
            s.conn(),
            &Sender::Human,
            ChannelArchiveCheckParams {
                channel_id: id.into(),
            },
            crate::testkit::NOW,
        )
    }
    #[test]
    fn legal_correction_does_not_hide_pending_or_unknown_judgments_and_history_is_not_capped() {
        use crewd_proto::judgments::*;
        let mut s = store();
        let ch = ready(&mut s);
        let record = JudgmentRecord {
            version: 1,
            decision_id: "fake-decision".into(),
            db_id: crewd_store::meta_get(s.conn(), "dbId").unwrap().unwrap(),
            restore_epoch: crewd_store::judgment_state::epoch(s.conn()).unwrap(),
            config_revision: "fixture".into(),
            source_version: "fixture".into(),
            phase: JudgmentPhase::Settled,
            prepared_at_ms: 0,
            captured_floors: BTreeMap::new(),
            captured_stop_epochs: BTreeMap::new(),
            recipients: vec![],
            attribution: None,
            diagnostic: serde_json::json!({}),
        };
        let extra = serde_json::json!({"judgmentV1":record}).to_string();
        let mut last = String::new();
        for _ in 0..260 {
            last = uuid::Uuid::new_v4().to_string();
            s.conn().execute("INSERT INTO messages(id,channel_id,sender_kind,text,created_at,extra) VALUES(?1,?2,'system','history','2026-10-31',?3)",params![last,ch,extra]).unwrap();
        }
        assert!(inspect(&s, &ch).unwrap().can_archive);
        for patch in [
            serde_json::json!({"version":99}),
            serde_json::json!({"phase":"unknown"}),
            serde_json::json!({"decisionId":null}),
            serde_json::json!({"capturedFloors":{"bad":-1}}),
        ] {
            let mut broken = serde_json::to_value(&record).unwrap();
            for (k, v) in patch.as_object().unwrap() {
                broken[k] = v.clone();
            }
            s.conn()
                .execute(
                    "UPDATE messages SET extra=?1 WHERE id=?2",
                    params![serde_json::json!({"judgmentV1":broken}).to_string(), last],
                )
                .unwrap();
            assert!(
                inspect(&s, &ch).is_err(),
                "unknown model authority must fail closed"
            );
        }
        let mut pending = record.clone();
        pending.phase = JudgmentPhase::Dispatched;
        let duplicate = format!(
            "{{\"judgmentV1\":{},\"judgmentV1\":{}}}",
            serde_json::to_string(&record).unwrap(),
            serde_json::to_string(&pending).unwrap()
        );
        s.conn()
            .execute(
                "UPDATE messages SET extra=?1 WHERE id=?2",
                params![duplicate, last],
            )
            .unwrap();
        assert_eq!(inspect(&s, &ch).unwrap().facts.pending_judgments, 1);
        s.conn()
            .execute(
                "UPDATE messages SET extra=?1 WHERE id=?2",
                params![serde_json::json!({"judgmentV1":pending}).to_string(), last],
            )
            .unwrap();
        let result = inspect(&s, &ch).unwrap();
        assert!(!result.can_archive);
        assert_eq!(result.facts.pending_judgments, 1);
    }
    #[test]
    fn corrupt_active_reservation_cannot_look_like_absence() {
        let mut s = store();
        let ch = ready(&mut s);
        for active in [
            serde_json::json!("broken"),
            serde_json::json!({"windows":null}),
            serde_json::json!({"windows":["bad"]}),
        ] {
            let value = serde_json::json!({"version":1,"sessionId":"w-a1","incarnation":uuid::Uuid::new_v4().to_string(),"revision":1,"active":active});
            crewd_store::meta_set(s.conn(), "cursor.reservation.v1.w-a1", &value.to_string())
                .unwrap();
            assert!(inspect(&s, &ch).is_err());
        }
    }

    #[test]
    fn duplicate_active_field_cannot_hide_last_active_reservation() {
        let mut s = store();
        let ch = ready(&mut s);
        let active = serde_json::json!({"token":uuid::Uuid::new_v4().to_string(),"executorGeneration":1,"phase":"held","windows":[{"channel":ch,"fromExclusive":0,"toInclusive":1}],"wakeIds":[],"riderIds":[]});
        let raw = format!(
            "{{\"version\":1,\"sessionId\":\"w-a1\",\"incarnation\":\"{}\",\"revision\":1,\"active\":null,\"active\":{active}}}",
            uuid::Uuid::new_v4()
        );
        crewd_store::meta_set(s.conn(), "cursor.reservation.v1.w-a1", &raw).unwrap();
        assert!(inspect(&s, &ch).is_err());
    }
    #[test]
    fn stale_pending_work_notice_does_not_block_but_inflight_still_does() {
        for (inflight, indirect) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut s = store();
            let ch = ready(&mut s);
            let sources = if indirect {
                let r = ok(&mut s, |tx| {
                    crate::requirements::create(
                        tx,
                        &Sender::Human,
                        crewd_proto::TodoCreateParams {
                            request_id: uuid::Uuid::new_v4().to_string(),
                            crew_id: "crew-a".into(),
                            channel_id: Some(ch.clone()),
                            text: "indirect work source".into(),
                        },
                    )
                });
                vec![r.todo.requirement_id.unwrap()]
            } else {
                vec![]
            };
            let w = ok(&mut s, |tx| {
                work::create(
                    tx,
                    &Sender::Human,
                    crewd_proto::WorkCreateParams {
                        crew_id: "crew-a".into(),
                        title: "assigned".into(),
                        request_id: Some(uuid::Uuid::new_v4().to_string()),
                        channel_id: (!indirect).then(|| ch.clone()),
                        sources: sources.clone(),
                        ..Default::default()
                    },
                )
            });
            let w = ok(&mut s, |tx| {
                work::assign(
                    tx,
                    &Sender::Human,
                    crewd_proto::WorkAssignParams {
                        id: w.id,
                        expected_revision: w.revision,
                        owner: "w-a1".into(),
                        note: "review original work".into(),
                    },
                )
            });
            let wake:i64=s.conn().query_row("SELECT id FROM wakes WHERE source LIKE 'work-assigned:%' ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
            if inflight {
                ok(&mut s, |tx| wakes::claim(tx, &[wake]));
            }
            ok(&mut s, |tx| {
                work::close_factual(
                    tx,
                    &Sender::Human,
                    crewd_proto::WorkCloseParams {
                        id: w.id,
                        expected_revision: w.revision,
                        kind: crewd_proto::WorkClosureKind::Done,
                        note: "checked".into(),
                        evidence: crewd_proto::WorkEvidenceParam {
                            kind: crewd_proto::WorkEvidenceKind::Text,
                            value: "proof".into(),
                        },
                    },
                )
            });
            let result = inspect(&s, &ch).unwrap();
            if inflight {
                assert!(!result.can_archive);
                assert_eq!(result.facts.inflight_wakes, 1);
                ok(&mut s, |tx| wakes::mark_delivered(tx, &[wake]));
                assert!(inspect(&s, &ch).unwrap().can_archive);
            } else {
                assert!(result.can_archive);
                assert!(result.facts.stale_wakes >= 1);
            }
        }
    }
    #[test]
    fn actual_open_ask_and_requirement_links_block_until_resolved() {
        let mut s = store();
        let ch = ready(&mut s);
        let task = ok(&mut s, |tx| {
            crate::todo_attributes::create_human(
                tx,
                &Sender::Human,
                crewd_proto::HumanTaskCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: ch.clone(),
                    text: "Human decision".into(),
                },
            )
        });
        let view = inspect(&s, &ch).unwrap();
        assert_eq!(view.facts.open_asks, 1);
        assert!(view.blockers.contains(&ChannelArchiveBlocker::OpenAsk));
        ok(&mut s, |tx| {
            crate::todo_attributes::set_state(
                tx,
                &Sender::Human,
                crewd_proto::TodoSetStateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    object: task.todo.object.clone(),
                    channel_id: ch.clone(),
                    expected_revision: task.todo.revision.clone(),
                    state: crewd_proto::ManualTodoState::Done,
                },
            )
        });
        assert_eq!(inspect(&s, &ch).unwrap().facts.open_asks, 0);
        let question = ok(&mut s, |tx| {
            asks::open(
                tx,
                "w-a1",
                asks::OpenAsk {
                    question: "Actual Agent question".into(),
                    ..Default::default()
                },
            )
        });
        assert_eq!(inspect(&s, "crew-a/human").unwrap().facts.open_asks, 1);
        let link_revision = crewd_store::channel_work::revision(s.conn(), &ch)
            .unwrap()
            .to_string();
        ok(&mut s, |tx| {
            crate::channel_work::link(
                tx,
                &Sender::Human,
                crewd_proto::ChannelWorkLinkParams {
                    channel_id: ch.clone(),
                    expected_link_revision: link_revision,
                    object: ChannelWorkObject {
                        kind: ChannelWorkKind::HumanTask,
                        id: question.ask.clone(),
                    },
                },
                true,
            )
        });
        assert_eq!(inspect(&s, &ch).unwrap().facts.open_asks, 1);
        ok(&mut s, |tx| {
            asks::answer(tx, "crew-a", &question.ask, "Human answer")
        });
        assert_eq!(inspect(&s, &ch).unwrap().facts.open_asks, 0);
        let req = ok(&mut s, |tx| {
            crate::requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: Some(ch.clone()),
                    text: "unresolved request".into(),
                },
            )
        });
        let view = inspect(&s, &ch).unwrap();
        assert_eq!(view.facts.open_requirements, 1);
        assert!(
            view.blockers
                .contains(&ChannelArchiveBlocker::OpenRequirement)
        );
        ok(&mut s, |tx| {
            crate::requirements::close(
                tx,
                &Sender::Human,
                crewd_proto::TodoCloseParams {
                    id: req.todo.requirement_id.unwrap(),
                    expected_status: req.todo.status,
                    note: "Human explicitly withdrew request".into(),
                },
            )
        });
        assert_eq!(inspect(&s, &ch).unwrap().facts.open_requirements, 0);
    }
    #[test]
    fn valid_active_reservation_only_blocks_its_actual_channel() {
        let mut s = store();
        let ch = ready(&mut s);
        s.conn().execute("INSERT INTO messages(id,channel_id,sender_kind,text,created_at) VALUES('reservation-message',?1,'system','actual window','2026-10-31')",[&ch]).unwrap();
        let end: i64 = s
            .conn()
            .query_row(
                "SELECT seq FROM messages WHERE id='reservation-message'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        for channel in [&ch, "crew-a/lobby"] {
            s.conn().execute("INSERT OR IGNORE INTO read_cursors(reader,channel_id,seq,updated_at) VALUES('w-a1',?1,0,'2026-10-31')",[channel]).unwrap();
        }
        let mut state = serde_json::json!({"version":1,"sessionId":"w-a1","incarnation":uuid::Uuid::new_v4().to_string(),"revision":1,"active":{"token":uuid::Uuid::new_v4().to_string(),"executorGeneration":1,"phase":"held","windows":[{"channel":ch,"fromExclusive":0,"toInclusive":end}],"wakeIds":[],"riderIds":[]}});
        crewd_store::meta_set(s.conn(), "cursor.reservation.v1.w-a1", &state.to_string()).unwrap();
        let view = inspect(&s, &ch).unwrap();
        assert_eq!(view.facts.active_deliveries, 1);
        assert!(!view.can_archive);
        state["active"]["windows"][0]["channel"] = serde_json::json!("crew-a/lobby");
        crewd_store::meta_set(s.conn(), "cursor.reservation.v1.w-a1", &state.to_string()).unwrap();
        let view = inspect(&s, &ch).unwrap();
        assert_eq!(view.facts.active_deliveries, 0);
        assert!(view.can_archive);
    }
    #[test]
    fn more_than_two_hundred_links_is_explicitly_incomplete() {
        let mut s = store();
        let ch = ready(&mut s);
        for n in 0..200 {
            ok(&mut s, |tx| {
                work::create(
                    tx,
                    &Sender::Human,
                    crewd_proto::WorkCreateParams {
                        crew_id: "crew-a".into(),
                        title: format!("bounded {n}"),
                        request_id: Some(uuid::Uuid::new_v4().to_string()),
                        channel_id: Some(ch.clone()),
                        ..Default::default()
                    },
                )
            });
        }
        let view = inspect(&s, &ch).unwrap();
        assert!(view.facts.incomplete);
        assert!(view.blockers.contains(&ChannelArchiveBlocker::Incomplete));
        assert!(!view.can_archive);
    }
    #[test]
    fn manual_archive_preserves_corrupt_optional_record_and_still_advances_generation() {
        let mut s = store();
        let ch = ready(&mut s);
        crewd_store::meta_set(s.conn(), &key(&ch), "{broken-action").unwrap();
        assert!(inspect(&s, &ch).is_err());
        ok(&mut s, |tx| {
            channels::update(
                tx,
                &Sender::Human,
                &ch,
                channels::ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        });
        let before =
            crewd_store::meta_get(s.conn(), &format!("channel.archive.generation.v1:{ch}"))
                .unwrap();
        ok(&mut s, |tx| {
            channels::update(
                tx,
                &Sender::Human,
                &ch,
                channels::ChannelPatch {
                    archived: Some(false),
                    ..Default::default()
                },
            )
        });
        assert_ne!(
            before,
            crewd_store::meta_get(s.conn(), &format!("channel.archive.generation.v1:{ch}"))
                .unwrap()
        );
        assert_eq!(
            crewd_store::meta_get(s.conn(), &key(&ch))
                .unwrap()
                .as_deref(),
            Some("{broken-action")
        );
        assert!(inspect(&s, &ch).is_err());
    }
    #[test]
    fn manual_done_requirement_does_not_hide_its_live_work() {
        let mut s = store();
        let ch = ready(&mut s);
        let req = ok(&mut s, |tx| {
            crate::requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: Some(ch.clone()),
                    text: "still executing".into(),
                },
            )
        });
        let id = req.todo.requirement_id.clone().unwrap();
        ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "unclosed source work".into(),
                    sources: vec![id.clone()],
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            crate::todo_attributes::set_state(
                tx,
                &Sender::Human,
                crewd_proto::TodoSetStateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel_id: ch.clone(),
                    object: ChannelWorkObject {
                        kind: ChannelWorkKind::Requirement,
                        id: id.clone(),
                    },
                    expected_revision: req.todo_attributes.revision,
                    state: crewd_proto::ManualTodoState::Done,
                },
            )
        });
        let view = inspect(&s, &ch).unwrap();
        assert_eq!(view.facts.open_requirements, 1);
        assert!(!view.can_archive);
        assert_eq!(view.facts.live_work, 1);
        let current = crate::requirements::get(s.conn(), &id).unwrap();
        assert!(
            crate::run(
                &mut s,
                crate::testkit::NOW,
                |tx| crate::requirements::close(
                    tx,
                    &Sender::Human,
                    crewd_proto::TodoCloseParams {
                        id,
                        expected_status: current.todo.status,
                        note: "explicit original requirement closure".into()
                    }
                )
            )
            .is_err()
        );
        assert_eq!(inspect(&s, &ch).unwrap().revision, view.revision);
    }
}
