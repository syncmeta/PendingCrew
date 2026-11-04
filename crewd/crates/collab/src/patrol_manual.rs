//! Human-authorized patrol jobs. They use the factual scanner independently of
//! scheduled runs and never trigger an executor or captain brief.
use crate::{Result, Tx, patrol_runtime};
use crewd_proto::{PatrolRun, PatrolStartDisposition, PatrolStartResult};
use crewd_store::{StoreError, judgment_state, meta_get, meta_set};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const PENDING: &str = "patrol.manual.pending.v1:";
const PENDING_END: &str = "patrol.manual.pending.v1;";
const CURSOR: &str = "patrol.manual.cursor.v1";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    epoch: String,
    crew: String,
    run: PatrolRun,
    retry_at_ms: Option<i64>,
}

fn key(crew: &str) -> String {
    format!("patrol.manual.run.v1:{crew}")
}

fn load(c: &Connection, crew: &str) -> Result<Option<Saved>> {
    let Some(raw) = meta_get(c, &key(crew))? else {
        return Ok(None);
    };
    let saved: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if saved.crew != crew || uuid::Uuid::parse_str(&saved.run.run_id).is_err() {
        return Err(StoreError::Invalid("invalid manual patrol identity".into()).into());
    }
    Ok((saved.epoch == judgment_state::epoch(c)?).then_some(saved))
}

fn save(c: &Connection, row: &Saved) -> Result<()> {
    meta_set(
        c,
        &key(&row.crew),
        &serde_json::to_string(row).map_err(StoreError::from)?,
    )?;
    Ok(())
}

pub fn start(tx: &mut Tx<'_>, crew: &str) -> Result<PatrolStartResult> {
    crate::crews::refuse_archived(tx.conn(), crew)?;
    let c = tx.conn();
    let pending = format!("{PENDING}{crew}");
    if let Some(saved) = load(c, crew)?
        && saved.run.completed_at_ms.is_none()
        && let Some(queued) = meta_get(c, &pending)?
    {
        if queued != saved.run.run_id {
            return Err(StoreError::Invalid("manual patrol queue identity mismatch".into()).into());
        }
        return Ok(PatrolStartResult {
            crew_id: crew.into(),
            run_id: saved.run.run_id,
            disposition: PatrolStartDisposition::Joined,
        });
    }
    let captured = c
        .query_row(
            "SELECT m.seq FROM messages m JOIN channels ch ON ch.id=m.channel_id
         WHERE ch.crew_id=?1 AND m.sender_kind!='system' ORDER BY m.seq DESC LIMIT 1",
            [crew],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    let saved = Saved {
        epoch: judgment_state::epoch(c)?,
        crew: crew.into(),
        run: patrol_runtime::new_run(tx.now_ms(), captured),
        retry_at_ms: None,
    };
    save(c, &saved)?;
    meta_set(c, &pending, &saved.run.run_id)?;
    Ok(PatrolStartResult {
        crew_id: crew.into(),
        run_id: saved.run.run_id,
        disposition: PatrolStartDisposition::Started,
    })
}

pub(super) enum Issue {
    Retrying,
    Interrupted,
}

pub(super) fn status(c: &Connection, crew: &str) -> Result<(Option<PatrolRun>, Option<Issue>)> {
    let Some(saved) = load(c, crew)? else {
        return Ok((None, None));
    };
    let issue = if saved.run.completed_at_ms.is_some() {
        None
    } else {
        let pending = meta_get(c, &format!("{PENDING}{crew}"))?;
        if pending.as_deref() != Some(&saved.run.run_id) {
            Some(Issue::Interrupted)
        } else {
            saved.retry_at_ms.map(|_| Issue::Retrying)
        }
    };
    Ok((Some(saved.run), issue))
}

/// Archival invalidates the job in the same transaction, including an
/// archive/unarchive pair between scheduler ticks. Preserve the prior receipt.
pub(crate) fn cancel(tx: &Tx<'_>, crew: &str) -> Result<()> {
    tx.conn().execute(
        "DELETE FROM meta WHERE key=?1",
        [format!("{PENDING}{crew}")],
    )?;
    Ok(())
}

pub fn has_pending(c: &Connection) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key>?1 AND key<?2)",
        params![PENDING, PENDING_END],
        |r| r.get(0),
    )?)
}

pub fn tick(tx: &mut Tx<'_>) -> Result<()> {
    let c = tx.conn();
    let cursor = meta_get(c, CURSOR)?.unwrap_or_default();
    let mut next =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT 1")?;
    let mut read = |after: &str| -> rusqlite::Result<Option<(String, String)>> {
        next.query_row(params![after, PENDING_END], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
    };
    let pending = match read(cursor.as_str().max(PENDING))? {
        None if cursor.as_str() > PENDING => read(PENDING)?,
        page => page,
    };
    let Some((pending, identity)) = pending else {
        return Ok(());
    };
    let Some(crew) = pending.strip_prefix(PENDING) else {
        return Ok(());
    };
    if cursor != pending {
        meta_set(c, CURSOR, &pending)?;
    }
    let active = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM crews WHERE id=?1 AND archived_at IS NULL)",
        [crew],
        |r| r.get::<_, bool>(0),
    )?;
    if !active {
        return cancel(tx, crew);
    }
    let mut saved = match load(c, crew) {
        Ok(Some(saved)) => saved,
        Ok(None) => return cancel(tx, crew),
        Err(_) => return Ok(()), // Preserve damaged record, continue round robin.
    };
    if saved.run.run_id != identity {
        return Ok(());
    }
    if saved.run.completed_at_ms.is_some() {
        return cancel(tx, crew);
    }
    if saved.retry_at_ms.is_some_and(|at| tx.now_ms() < at) {
        return Ok(());
    }
    let mut draft = saved.run.clone();
    if patrol_runtime::scan_step(tx, crew, &mut draft).is_ok() {
        saved.run = draft;
        saved.retry_at_ms = None;
    } else {
        saved.retry_at_ms = Some(tx.now_ms().saturating_add(60_000));
    }
    save(c, &saved)?;
    if saved.run.completed_at_ms.is_some() {
        cancel(tx, crew)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok, store};
    use crewd_proto::WorkCreateParams;
    use crewd_store::Store;

    fn work(s: &mut Store, crew: &str) -> String {
        ok(s, |tx| {
            crate::work::create(
                tx,
                &crate::Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: "isolated patrol fixture".into(),
                    ..Default::default()
                },
            )
        })
        .id
    }

    #[test]
    fn manual_patrol_progress_is_bounded_independent_and_resumes_after_file_reopen() {
        let mut s = store();
        let local: Vec<_> = (0..27).map(|_| work(&mut s, "crew-a")).collect();
        let foreign = work(&mut s, "crew-b");
        for _ in 0..3 {
            ok(&mut s, patrol_runtime::tick);
        }
        let auto = patrol_runtime::status(s.conn(), "crew-a")
            .unwrap()
            .run
            .unwrap();
        let first = ok(&mut s, |tx| start(tx, "crew-a"));
        assert_eq!(
            patrol_runtime::status(s.conn(), "crew-a")
                .unwrap()
                .manual_run_active,
            Some(true)
        );
        let joined = ok(&mut s, |tx| start(tx, "crew-a"));
        assert_eq!(joined.run_id, first.run_id);
        assert_eq!(joined.disposition, PatrolStartDisposition::Joined);
        ok(&mut s, tick);
        let partial = status(s.conn(), "crew-a").unwrap().0.unwrap();
        assert_eq!(partial.scanned_counts["work"], 25);
        assert!(partial.after.is_some() && partial.completed_at_ms.is_none());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let mut reopened = Store::open(&path).unwrap();
        for _ in 0..4 {
            ok(&mut reopened, tick);
        }
        let result = patrol_runtime::status(reopened.conn(), "crew-a").unwrap();
        assert_eq!(result.manual_run_active, Some(false));
        let manual = result.manual_run.unwrap();
        assert_eq!(manual.run_id, first.run_id);
        assert_eq!(manual.completed_at_ms, Some(NOW));
        assert_eq!(manual.scanned_counts["work"], 27);
        assert_eq!(manual.findings["unowned"], local);
        assert!(!manual.findings["unowned"].contains(&foreign));
        assert_eq!(
            serde_json::to_value(result.run.unwrap()).unwrap(),
            serde_json::to_value(auto).unwrap()
        );
        assert!(!has_pending(reopened.conn()).unwrap());
        assert!(
            crate::wakes::list(reopened.conn(), None, 10)
                .unwrap()
                .is_empty()
        );
        let second = ok(&mut reopened, |tx| start(tx, "crew-a"));
        assert_ne!(second.run_id, first.run_id);
        assert_eq!(second.disposition, PatrolStartDisposition::Started);
    }

    #[test]
    fn manual_patrol_fences_archive_aba_restore_and_damaged_queue() {
        let mut s = store();
        let first = ok(&mut s, |tx| start(tx, "crew-x"));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crate::crews::unarchive(tx, "crew-x"));
        let (interrupted, issue) = status(s.conn(), "crew-x").unwrap();
        assert_eq!(interrupted.unwrap().run_id, first.run_id);
        assert!(matches!(issue, Some(Issue::Interrupted)));
        assert_eq!(
            patrol_runtime::status(s.conn(), "crew-x")
                .unwrap()
                .manual_run_active,
            Some(false)
        );
        let replacement = ok(&mut s, |tx| start(tx, "crew-x"));
        assert_ne!(replacement.run_id, first.run_id);

        ok(&mut s, |tx| start(tx, "crew-a"));
        meta_set(s.conn(), &format!("{PENDING}crew-a"), "damaged").unwrap();
        ok(&mut s, |tx| start(tx, "crew-b"));
        for _ in 0..10 {
            ok(&mut s, tick);
        }
        assert!(
            status(s.conn(), "crew-b")
                .unwrap()
                .0
                .unwrap()
                .completed_at_ms
                .is_some()
        );
        assert!(matches!(
            status(s.conn(), "crew-a").unwrap().1,
            Some(Issue::Interrupted)
        ));
        assert_eq!(
            meta_get(s.conn(), &format!("{PENDING}crew-a"))
                .unwrap()
                .as_deref(),
            Some("damaged")
        );

        ok(&mut s, |tx| Ok(judgment_state::restored(tx.conn())?));
        assert!(status(s.conn(), "crew-x").unwrap().0.is_none());
        for _ in 0..4 {
            ok(&mut s, tick);
        }
        assert!(!has_pending(s.conn()).unwrap());
    }
}
