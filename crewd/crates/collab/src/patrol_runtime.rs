//! One crew/source page per tick. Observations have no wake or execution effects.
use crate::{Result, Tx, channel_archive_discovery as archive};
use crewd_proto::{PatrolRun, PatrolStatus, WorkPatrolParams, WorkPatrolScan};
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
const CURSOR: &str = "patrol.cursor.v1";
const ARCHIVE_CHECKS_PER_STEP: u8 = 2;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    epoch: String,
    run: PatrolRun,
}
#[derive(Serialize, Deserialize)]
struct ErrorSaved {
    epoch: String,
    retry_at: i64,
}
fn error_saved(c: &Connection, crew: &str) -> Result<Option<ErrorSaved>> {
    let raw = meta_get(c, &format!("patrol.error.v1:{crew}"))?;
    let error = raw
        .map(|v| serde_json::from_str::<ErrorSaved>(&v))
        .transpose()
        .map_err(StoreError::from)?;
    Ok(error.filter(|e| crewd_store::judgment_state::epoch(c).is_ok_and(|epoch| e.epoch == epoch)))
}
fn key(crew: &str) -> String {
    format!("patrol.run.v1:{crew}")
}
fn load(c: &Connection, crew: &str) -> Result<Option<Saved>> {
    let Some(raw) = meta_get(c, &key(crew))? else {
        return Ok(None);
    };
    let saved: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if saved.version != 1 {
        return Err(StoreError::Invalid("unsupported patrol state".into()).into());
    }
    Ok((saved.epoch == crewd_store::judgment_state::epoch(c)?).then_some(saved))
}
pub fn status(c: &Connection, crew: &str) -> Result<PatrolStatus> {
    crate::crews::refuse_archived(c, crew)?;
    let observed = load(c, crew);
    let error = if observed.is_err() {
        Some("巡检状态不可读取".into())
    } else {
        match error_saved(c, crew) {
            Ok(e) => e.map(|_| "巡检读取失败；原进度保留，稍后重试".into()),
            Err(_) => Some("巡检错误记录不可读取；原记录已保留".into()),
        }
    };
    Ok(PatrolStatus {
        crew_id: crew.into(),
        run: observed.ok().flatten().map(|s| s.run),
        error,
    })
}
fn finding(r: &mut PatrolRun, category: &str, id: String) {
    *r.counts.entry(category.into()).or_default() += 1;
    let ids = r.findings.entry(category.into()).or_default();
    if ids.len() < 50 {
        ids.push(id)
    } else {
        r.truncated = true
    }
}
pub fn tick(tx: &mut Tx<'_>) -> Result<()> {
    let cursor = meta_get(tx.conn(), CURSOR)?.unwrap_or_default();
    let crew: Option<String> = tx
        .conn()
        .query_row(
            "SELECT id FROM crews WHERE archived_at IS NULL AND id>?1 ORDER BY id LIMIT 1",
            [&cursor],
            |r| r.get(0),
        )
        .optional()?;
    let Some(crew) = crew else {
        meta_set(tx.conn(), CURSOR, "")?;
        return Ok(());
    };
    meta_set(tx.conn(), CURSOR, &crew)?;
    let error_key = format!("patrol.error.v1:{crew}");
    match error_saved(tx.conn(), &crew) {
        Err(_) => return Ok(()), // Preserve corrupt payload and committed round-robin progress.
        Ok(Some(e)) if tx.now_ms() < e.retry_at => return Ok(()),
        _ => {}
    }
    tx.conn().execute_batch("SAVEPOINT patrol_one")?;
    match step(tx, &crew) {
        Ok(()) => {
            tx.conn().execute_batch("RELEASE patrol_one")?;
            tx.conn()
                .execute("DELETE FROM meta WHERE key=?1", [&error_key])?;
        }
        Err(_) => {
            tx.conn()
                .execute_batch("ROLLBACK TO patrol_one; RELEASE patrol_one")?;
            let e = ErrorSaved {
                epoch: crewd_store::judgment_state::epoch(tx.conn())?,
                retry_at: tx.now_ms().saturating_add(60_000),
            };
            meta_set(
                tx.conn(),
                &error_key,
                &serde_json::to_string(&e).map_err(StoreError::from)?,
            )?;
        }
    }
    Ok(())
}
fn step(tx: &mut Tx<'_>, crew: &str) -> Result<()> {
    let mut saved = load(tx.conn(), crew)?;
    let baseline = saved.as_ref().map(|s| s.run.captured_seq).unwrap_or(0);
    let (count,seq):(u32,i64)=tx.conn().query_row("SELECT count(*),coalesce(max(seq),?2) FROM (SELECT m.seq FROM messages m JOIN channels ch ON ch.id=m.channel_id WHERE ch.crew_id=?1 AND m.seq>?2 AND m.sender_kind!='system' ORDER BY m.seq DESC LIMIT 10)",rusqlite::params![crew,baseline],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let complete = saved.as_ref().and_then(|s| s.run.completed_at_ms);
    if saved.is_none() || complete.is_some() {
        if !crewd_work::patrol_due(
            count,
            complete.map(crewd_work::Ts),
            crewd_work::Ts(tx.now_ms()),
            &crewd_work::Config::default(),
        ) {
            return Ok(());
        }
        saved = Some(Saved {
            version: 1,
            epoch: crewd_store::judgment_state::epoch(tx.conn())?,
            run: PatrolRun {
                run_id: uuid::Uuid::new_v4().to_string(),
                started_at_ms: tx.now_ms(),
                completed_at_ms: None,
                scan: WorkPatrolScan::Work,
                after: None,
                captured_seq: seq,
                scanned_counts: BTreeMap::new(),
                findings: BTreeMap::new(),
                counts: BTreeMap::new(),
                truncated: false,
            },
        });
    }
    let saved = saved.as_mut().expect("initialized");
    let r = &mut saved.run;
    let page = crate::work_patrol::read(
        tx.conn(),
        WorkPatrolParams {
            crew_id: Some(crew.into()),
            scan: r.scan,
            after: r.after.clone(),
            limit: Some(25),
        },
        tx.now_ms(),
    )?;
    *r.scanned_counts
        .entry(format!("{:?}", r.scan).to_lowercase())
        .or_default() += (page.items.len() + page.entries.len()) as u64;
    for i in page.items {
        if i.unowned {
            finding(r, "unowned", i.id.clone())
        }
        if i.lost.is_some() {
            finding(r, "lost", i.id.clone())
        }
        if i.stop_escalated {
            finding(r, "stop_escalated", i.id.clone())
        }
        if !i.long_waits.is_empty() {
            finding(r, "long_waits", i.id)
        }
    }
    for i in page.entries {
        if i.unclaimed {
            finding(r, "unclaimed", i.id.clone())
        }
        if i.needs_ruling {
            finding(r, "needs_ruling", i.id)
        }
    }
    r.after = page.next_after;
    let mut verified = 0;
    let mut checkpoint = None;
    for i in page.channels {
        let now_ms = tx.now_ms();
        let review = match archive::old_enough(tx.conn(), &i, now_ms) {
            Ok(true) if verified == ARCHIVE_CHECKS_PER_STEP => {
                // Do not consume or count this row. The next round resumes
                // immediately after the last row actually examined.
                r.after = checkpoint;
                break;
            }
            Ok(true) => {
                verified += 1;
                archive::verify(tx.conn(), &i, now_ms)
            }
            Ok(false) => Ok(archive::Review::Ineligible),
            Err(error) => Err(error),
        };
        checkpoint = Some(i.id.clone());
        *r.scanned_counts.entry("channels".into()).or_default() += 1;
        if i.blocked_by
            .contains(&crewd_proto::WorkPatrolChannelBlocker::LinksTruncated)
        {
            finding(r, "incomplete_channels", i.id.clone());
        }
        if i.archive_candidate {
            finding(r, "archive_candidates", i.id.clone());
        }
        match review {
            Ok(archive::Review::Candidate) => finding(r, "idle_archive_candidates", i.id),
            Ok(archive::Review::Incomplete) | Err(_) => {
                finding(r, "archive_review_incomplete", i.id)
            }
            Ok(archive::Review::Ineligible) => {}
        }
    }
    if r.after.is_none() {
        match r.scan {
            WorkPatrolScan::Work => r.scan = WorkPatrolScan::Entries,
            WorkPatrolScan::Entries => r.scan = WorkPatrolScan::Channels,
            WorkPatrolScan::Channels => r.completed_at_ms = Some(tx.now_ms()),
        }
    }
    meta_set(
        tx.conn(),
        &key(crew),
        &serde_json::to_string(saved).map_err(StoreError::from)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender, channels, requirements,
        testkit::{NOW, ok, store},
        todo_attributes, work,
    };
    use crewd_proto::{
        ChannelKind, HumanTaskCreateParams, ManualTodoState, TodoCreateParams, TodoSetStateParams,
        WorkCloseParams, WorkClosureKind, WorkCreateParams, WorkEvidenceKind, WorkEvidenceParam,
    };
    use crewd_store::Store;

    fn topic(s: &mut Store) -> String {
        ok(s, |tx| {
            channels::create(
                tx,
                &Sender::Human,
                channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "archive review fixture".into(),
                    ..Default::default()
                },
            )
        })
        .id
    }

    fn finished_work(s: &mut Store, channel: Option<String>, sources: Vec<String>) {
        let work = ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    request_id: channel.as_ref().map(|_| uuid::Uuid::new_v4().to_string()),
                    channel_id: channel,
                    crew_id: "crew-a".into(),
                    title: "finished archive review fixture".into(),
                    sources,
                    ..Default::default()
                },
            )
        });
        ok(s, |tx| {
            work::close_factual(
                tx,
                &Sender::Human,
                WorkCloseParams {
                    id: work.id,
                    expected_revision: work.revision,
                    kind: WorkClosureKind::Done,
                    note: "checked".into(),
                    evidence: WorkEvidenceParam {
                        kind: WorkEvidenceKind::Text,
                        value: "finished".into(),
                    },
                },
            )
        });
    }

    fn message(s: &Store, channel: &str, at: &str) {
        s.conn().execute(
            "INSERT INTO messages(id,channel_id,sender_kind,text,created_at) VALUES(?1,?2,'human','fixture',?3)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), channel, at],
        ).unwrap();
    }

    fn advance(s: &mut Store) -> PatrolRun {
        ok(s, |tx| step(tx, "crew-a"));
        status(s.conn(), "crew-a").unwrap().run.unwrap()
    }

    #[test]
    fn archive_checks_continue_from_the_last_consumed_channel_without_duplicates() {
        let mut s = store();
        let mut old = Vec::new();
        for _ in 0..5 {
            let id = topic(&mut s);
            finished_work(&mut s, Some(id.clone()), Vec::new());
            message(&s, &id, &crewd_store::ts_from_ms(NOW - archive::IDLE_MS));
            old.push(id);
        }
        old.sort();
        let fresh = topic(&mut s);
        finished_work(&mut s, Some(fresh.clone()), Vec::new());
        message(
            &s,
            &fresh,
            &crewd_store::ts_from_ms(NOW - archive::IDLE_MS + 1),
        );
        let broken = &old[0];
        let bad_key = format!("channel.archive.v1:{broken}");
        meta_set(s.conn(), &bad_key, "{broken retained archive history").unwrap();

        assert_eq!(advance(&mut s).scan, WorkPatrolScan::Entries);
        assert_eq!(advance(&mut s).scan, WorkPatrolScan::Channels);
        let first = advance(&mut s);
        assert!(
            first
                .after
                .as_deref()
                .is_some_and(|id| id < old[2].as_str())
        );
        assert_eq!(first.findings["idle_archive_candidates"], old[1..2]);
        assert_eq!(
            first.findings["archive_review_incomplete"],
            [broken.clone()]
        );
        let second = advance(&mut s);
        assert!(second.after.is_some());
        assert_eq!(second.findings["idle_archive_candidates"], old[1..4]);
        let done = advance(&mut s);
        assert!(done.completed_at_ms.is_some());
        assert_eq!(done.scanned_counts["channels"], 6);
        assert_eq!(done.findings["idle_archive_candidates"], old[1..]);
        assert_eq!(done.counts["archive_candidates"], 6);
        assert_eq!(done.counts["archive_review_incomplete"], 1);
        assert!(
            channels::get(s.conn(), &old[1])
                .unwrap()
                .archived_at
                .is_none()
        );
        assert_eq!(
            meta_get(s.conn(), &bad_key).unwrap().as_deref(),
            Some("{broken retained archive history")
        );
    }

    #[test]
    fn idle_topic_with_requirement_link_and_completed_human_task_uses_canonical_facts() {
        let mut s = store();
        let id = topic(&mut s);
        let requirement = ok(&mut s, |tx| {
            requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: Some(id.clone()),
                    text: "completed original request".into(),
                },
            )
        })
        .todo
        .requirement_id
        .unwrap();
        finished_work(&mut s, None, vec![requirement]);
        let ask = ok(&mut s, |tx| {
            todo_attributes::create_human(
                tx,
                &Sender::Human,
                HumanTaskCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: id.clone(),
                    text: "final human check".into(),
                },
            )
        });
        message(&s, &id, &crewd_store::ts_from_ms(NOW - archive::IDLE_MS));
        advance(&mut s);
        advance(&mut s);
        let still_open = advance(&mut s);
        assert!(still_open.completed_at_ms.is_some());
        assert!(!still_open.findings.contains_key("idle_archive_candidates"));
        ok(&mut s, |tx| {
            todo_attributes::set_state(
                tx,
                &Sender::Human,
                TodoSetStateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    object: ask.todo.object,
                    channel_id: id.clone(),
                    expected_revision: ask.todo.revision,
                    state: ManualTodoState::Done,
                },
            )
        });
        for _ in 0..3 {
            crate::testkit::ok_at(&mut s, NOW + 30 * 60 * 1000, |tx| step(tx, "crew-a"));
        }
        let candidate = status(s.conn(), "crew-a").unwrap().run.unwrap();
        assert!(candidate.completed_at_ms.is_some());
        assert!(!candidate.findings.contains_key("archive_candidates"));
        assert_eq!(candidate.findings["idle_archive_candidates"], [id]);
    }
    #[test]
    fn corrupt_error_metadata_is_visible_preserved_and_does_not_starve() {
        let mut s = store();
        meta_set(s.conn(), "patrol.error.v1:crew-a", "invalid").unwrap();
        for _ in 0..30 {
            ok(&mut s, |tx| tick(tx));
        }
        assert_eq!(
            meta_get(s.conn(), "patrol.error.v1:crew-a")
                .unwrap()
                .as_deref(),
            Some("invalid")
        );
        assert!(status(s.conn(), "crew-a").unwrap().error.is_some());
        assert!(
            status(s.conn(), "crew-b")
                .unwrap()
                .run
                .unwrap()
                .completed_at_ms
                .is_some()
        );
    }
    #[test]
    fn truncated_links_remain_an_explicit_incomplete_finding() {
        let mut s = store();
        s.conn().execute("INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES('patrol-incomplete','crew-b','topic','test','human','2026-01-01')",[]).unwrap();
        s.conn().execute("INSERT INTO channel_work_scopes(channel_id,revision) VALUES('patrol-incomplete',1)",[]).unwrap();
        for n in 1..=201 {
            s.conn().execute("INSERT INTO channel_work_links(channel_id,object_kind,object_id,source_crew_id) VALUES('patrol-incomplete','requirement',?1,'crew-b')",[format!("#{n}")]).unwrap();
        }
        for _ in 0..30 {
            ok(&mut s, |tx| tick(tx));
        }
        let run = status(s.conn(), "crew-b").unwrap().run.unwrap();
        assert!(run.completed_at_ms.is_some());
        assert_eq!(run.counts["incomplete_channels"], 1);
        assert_eq!(run.findings["incomplete_channels"], ["patrol-incomplete"]);
        assert!(!run.truncated);
    }
    #[test]
    fn ten_messages_trigger_and_inflight_messages_are_not_consumed() {
        let mut s = store();
        for _ in 0..30 {
            ok(&mut s, |tx| tick(tx));
        }
        let initial = status(s.conn(), "crew-b").unwrap().run.unwrap().run_id;
        let channel: String = s
            .conn()
            .query_row(
                "SELECT id FROM channels WHERE crew_id='crew-b' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let add = |s: &crewd_store::Store, n: usize| {
            for _ in 0..n {
                s.conn().execute("INSERT INTO messages(id,channel_id,sender_kind,text,created_at) VALUES(?1,?2,'human','hello','2026-01-01')",rusqlite::params![uuid::Uuid::new_v4().to_string(),channel]).unwrap();
            }
        };
        add(&s, 9);
        for _ in 0..10 {
            ok(&mut s, |tx| tick(tx));
        }
        assert_eq!(
            status(s.conn(), "crew-b").unwrap().run.unwrap().run_id,
            initial
        );
        add(&s, 1);
        for _ in 0..10 {
            ok(&mut s, |tx| tick(tx));
            if status(s.conn(), "crew-b").unwrap().run.unwrap().run_id != initial {
                break;
            }
        }
        let active = status(s.conn(), "crew-b").unwrap().run.unwrap();
        assert!(active.completed_at_ms.is_none());
        add(&s, 10);
        for _ in 0..50 {
            ok(&mut s, |tx| tick(tx));
        }
        assert_ne!(
            status(s.conn(), "crew-b").unwrap().run.unwrap().run_id,
            active.run_id
        );
    }
    #[test]
    fn completed_observation_waits_for_interval_and_restore_invalidates_it() {
        let mut s = store();
        for _ in 0..30 {
            ok(&mut s, |tx| tick(tx));
        }
        let before = status(s.conn(), "crew-b").unwrap().run.unwrap();
        assert!(before.completed_at_ms.is_some());
        for _ in 0..30 {
            ok(&mut s, |tx| tick(tx));
        }
        assert_eq!(
            status(s.conn(), "crew-b").unwrap().run.unwrap().run_id,
            before.run_id
        );
        for _ in 0..30 {
            crate::testkit::ok_at(
                &mut s,
                before.completed_at_ms.unwrap() + 30 * 60 * 1000,
                |tx| tick(tx),
            );
        }
        assert_ne!(
            status(s.conn(), "crew-b").unwrap().run.unwrap().run_id,
            before.run_id
        );
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(status(s.conn(), "crew-b").unwrap().run.is_none());
    }
    #[test]
    fn broken_crew_cannot_starve_next_and_payload_is_preserved() {
        let mut s = store();
        meta_set(s.conn(), &key("crew-a"), "invalid").unwrap();
        for _ in 0..30 {
            ok(&mut s, |tx| tick(tx));
        }
        assert_eq!(
            meta_get(s.conn(), &key("crew-a")).unwrap().as_deref(),
            Some("invalid")
        );
        assert!(status(s.conn(), "crew-a").unwrap().error.is_some());
        assert!(
            status(s.conn(), "crew-b")
                .unwrap()
                .run
                .unwrap()
                .completed_at_ms
                .is_some()
        );
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(error_saved(s.conn(), "crew-a").unwrap().is_none());
    }
}
