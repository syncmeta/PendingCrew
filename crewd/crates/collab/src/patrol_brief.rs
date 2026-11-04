//! Deliver selected facts from a completed patrol to its live, admitted captain.
use crate::{Result, Tx, crews, wakes, work::FollowupNote};
use crewd_proto::{Desired, PatrolCaptainBrief, PatrolCaptainBriefPhase, PatrolRun, Role};
use crewd_store::{StoreError, judgment_state, meta_get, meta_set, object_refs};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

const SOURCE: &str = "patrol-brief:";
const INTERVAL_MS: i64 = 30 * 60 * 1_000;
const MAX_SAMPLE: usize = 8;
const CATEGORIES: [(&str, &str, &str); 7] = [
    ("unowned", "无人负责的工作", "Unowned work"),
    ("lost", "负责人失联的工作", "Work with a lost owner"),
    ("stop_escalated", "停止检查已升级", "Escalated stop checks"),
    (
        "long_waits",
        "等待人类决定的工作",
        "Work awaiting a Human decision",
    ),
    ("unclaimed", "尚未认领的需求", "Unclaimed requirements"),
    (
        "needs_ruling",
        "需要裁定的需求",
        "Requirements awaiting a ruling",
    ),
    (
        "idle_archive_candidates",
        "可复查归档条件的群",
        "Channels to review for archival",
    ),
];

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Target {
    run: String,
    crew: String,
    captain: String,
    generation: String,
    epoch: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Dispatch {
    target: Target,
    wake: i64,
    at: i64,
    fingerprint: String,
    last_clear: Option<String>,
    #[serde(default)]
    claimed: bool,
}

fn key(crew: &str) -> String {
    format!("patrol.brief.v1:{crew}")
}

fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    Ok(serde_json::from_str(raw).map_err(StoreError::from)?)
}

fn encode(value: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(value).map_err(StoreError::from)?)
}

fn load(c: &Connection, crew: &str) -> Result<Option<Dispatch>> {
    let previous: Option<Dispatch> = meta_get(c, &key(crew))?
        .as_deref()
        .map(decode)
        .transpose()?;
    let epoch = judgment_state::epoch(c)?;
    Ok(previous.filter(|row| row.target.epoch == epoch && row.target.crew == crew))
}

fn current_captain(c: &Connection, crew: &str, captain: &str) -> Result<bool> {
    Ok(crews::captain_of(c, crew)?.as_deref() == Some(captain)
        && !crews::is_archived(c, crew)?
        && crewd_store::sessions::get(c, captain)?
            .is_some_and(|row| row.crew_id == crew && row.role == Role::Captain))
}

type Findings<'a> = Vec<(usize, u64, &'a [String])>;

fn findings(run: &PatrolRun) -> Findings<'_> {
    CATEGORIES
        .iter()
        .enumerate()
        .filter_map(|(i, (key, _, _))| {
            let count = run.counts.get(*key).copied()?;
            let refs = run.findings.get(*key)?;
            (count > 0 && !refs.is_empty()).then_some((i, count, refs.as_slice()))
        })
        .collect()
}

fn valid_samples(c: &Connection, crew: &str, findings: &Findings<'_>) -> Result<bool> {
    use object_refs::ObjectKind;
    for &(i, count, refs) in findings {
        if refs.len() > 50 || refs.len() as u64 > count {
            return Ok(false);
        }
        let kind = match CATEGORIES[i].0 {
            "idle_archive_candidates" => None,
            "unclaimed" | "needs_ruling" => Some(ObjectKind::Requirement),
            _ => Some(ObjectKind::Plan),
        };
        for id in refs.iter().take(MAX_SAMPLE) {
            let local = if let Some(kind) = kind {
                id.strip_prefix(kind.prefix())
                    .and_then(|n| n.parse::<i64>().ok())
                    .filter(|n| *n > 0)
                    .map(|n| object_refs::get(c, kind, n))
                    .transpose()?
                    .flatten()
                    .is_some_and(|r| r.crew_id == crew && r.id() == *id)
            } else {
                id.len() <= 256 && !id.chars().any(char::is_control)
                    && c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM channels WHERE id=?1 AND crew_id=?2 AND kind='topic')",
                        rusqlite::params![id, crew], |row| row.get::<_, bool>(0),
                    )?
            };
            if !local {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(crate) fn enqueue(
    tx: &mut Tx<'_>,
    crew: &str,
    captain: &str,
    generation: &str,
    run: &PatrolRun,
) -> Result<()> {
    let c = tx.conn();
    let Some(completed) = run.completed_at_ms else {
        return Ok(());
    };
    if generation.is_empty()
        || completed > tx.now_ms()
        || uuid::Uuid::parse_str(&run.run_id).is_err()
        || !current_captain(c, crew, captain)?
        || !crewd_store::sessions::get(c, captain)?.is_some_and(|s| s.desired == Desired::Running)
        || wakes::automatic_work_paused(c, captain)?
    {
        return Ok(());
    }
    let report = findings(run);
    let previous = load(c, crew)?;
    if report.is_empty() {
        if let Some(mut previous) = previous
            && previous.last_clear.as_deref() != Some(&run.run_id)
        {
            previous.last_clear = Some(run.run_id.clone());
            meta_set(c, &key(crew), &encode(&previous)?)?;
        }
        return Ok(());
    }
    if !valid_samples(c, crew, &report)? {
        return Ok(());
    }
    let digest = crate::judgments::fingerprint(&report)?;
    if let Some(previous) = previous {
        // The retained dispatch prevents duplicate delivery even after wake pruning.
        if previous.target.run == run.run_id
            || tx.now_ms().saturating_sub(previous.at) < INTERVAL_MS
            || (previous.target.captain == captain
                && previous.last_clear.is_none()
                && previous.fingerprint == digest)
            || wakes::by_ids(c, &[previous.wake])?
                .first()
                .is_some_and(|wake| {
                    matches!(
                        wake.state,
                        wakes::WakeState::Pending | wakes::WakeState::Inflight
                    )
                })
        {
            return Ok(());
        }
    }
    let target = Target {
        run: run.run_id.clone(),
        crew: crew.into(),
        captain: captain.into(),
        generation: generation.into(),
        epoch: judgment_state::epoch(c)?,
    };
    let note = encode(&target)?;
    let wake = wakes::enqueue(
        tx,
        wakes::WakeRequest {
            session: captain.into(),
            kind: wakes::WakeKind::System,
            source: format!("{SOURCE}{}", run.run_id),
            priority: wakes::WakePriority::Auto,
            not_before_ms: None,
            note: Some(note),
            origin: None,
        },
    )?;
    let wakes::Enqueued::New(wake) = wake else {
        return Ok(());
    };
    let row = Dispatch {
        target,
        wake,
        at: tx.now_ms(),
        fingerprint: digest,
        last_clear: None,
        claimed: false,
    };
    meta_set(tx.conn(), &key(crew), &encode(&row)?)?;
    Ok(())
}

pub fn is_brief(source: &str) -> bool {
    source.starts_with(SOURCE)
}

pub fn matches_generation(source: &str, note: Option<&str>, generation: &str) -> bool {
    note.and_then(|note| decode::<Target>(note).ok())
        .is_some_and(|target| {
            source.strip_prefix(SOURCE) == Some(&target.run) && target.generation == generation
        })
}

fn owns_wake(row: &Dispatch, wake: &wakes::WakeRow) -> bool {
    wake.id == row.wake
        && wake.session == row.target.captain
        && wake.kind == wakes::WakeKind::System
        && wake.priority == wakes::WakePriority::Auto
        && wake.source.strip_prefix(SOURCE) == Some(&row.target.run)
        && wake
            .note
            .as_deref()
            .and_then(|raw| decode::<Target>(raw).ok())
            .as_ref()
            == Some(&row.target)
}

pub(crate) fn status(
    c: &Connection,
    crew: &str,
    run: &PatrolRun,
) -> Result<Option<PatrolCaptainBrief>> {
    use PatrolCaptainBriefPhase as Phase;
    let Some(dispatch) = load(c, crew)? else {
        return Ok(None);
    };
    if dispatch.target.run != run.run_id || !current_captain(c, crew, &dispatch.target.captain)? {
        return Ok(None);
    }
    let phase = match wakes::by_ids(c, &[dispatch.wake])?
        .first()
        .filter(|wake| owns_wake(&dispatch, wake))
    {
        Some(wake) => match (wake.state, dispatch.claimed || wake.attempts > 0) {
            (wakes::WakeState::Pending, false) => Phase::Queued,
            (wakes::WakeState::Inflight, _) => Phase::Inflight,
            (wakes::WakeState::Delivered, _) => Phase::Delivered,
            (wakes::WakeState::Dropped, false) => Phase::Dropped,
            _ => Phase::Unknown,
        },
        None => Phase::Unknown,
    };
    Ok(Some(PatrolCaptainBrief {
        run_id: run.run_id.clone(),
        captain_session_id: dispatch.target.captain,
        wake_id: dispatch.wake,
        phase,
    }))
}

pub(crate) fn render(c: &Connection, wake: &wakes::WakeRow) -> Result<FollowupNote> {
    let Some(target) = wake
        .note
        .as_deref()
        .and_then(|note| decode::<Target>(note).ok())
    else {
        return Ok(FollowupNote::Stale);
    };
    let Some(dispatch) = load(c, &target.crew)? else {
        return Ok(FollowupNote::Stale);
    };
    if !owns_wake(&dispatch, wake)
        || dispatch.claimed
        || wake.attempts > 0
        || wakes::human_stop_floor(c, &wake.session)? >= wake.id
        || !current_captain(c, &target.crew, &target.captain)?
        || !crewd_store::sessions::get(c, &target.captain)?
            .is_some_and(|s| s.desired == Desired::Running)
    {
        return Ok(FollowupNote::Stale);
    }
    let Some(run) = crate::patrol_runtime::run(c, &target.crew)? else {
        return Ok(FollowupNote::Stale);
    };
    let Some(completed) = run.completed_at_ms.filter(|_| run.run_id == target.run) else {
        return Ok(FollowupNote::Stale);
    };
    let report = findings(&run);
    if report.is_empty()
        || crate::judgments::fingerprint(&report)? != dispatch.fingerprint
        || !valid_samples(c, &target.crew, &report)?
    {
        return Ok(FollowupNote::Stale);
    }
    let chinese = crate::settings::locale(c)? == crate::Locale::Zh;
    let time = crewd_store::ts_from_ms(completed);
    let mut text = if chinese {
        format!("本机组行政巡检简报（观察结束于 {time}）。以下为历史观察，处理前请核对当前事实。")
    } else {
        format!(
            "Administrative patrol brief for this crew (observed through {time}). Check current facts before taking action."
        )
    };
    for (i, count, refs) in report {
        let (_, zh, en) = CATEGORIES[i];
        let sample = &refs[..refs.len().min(MAX_SAMPLE)];
        let label = if chinese { zh } else { en };
        let detail = if (sample.len() as u64) < count {
            " …"
        } else {
            ""
        };
        text.push_str(&format!(
            "\n{label} ({count}): {}{detail}",
            sample.join(", ")
        ));
    }
    Ok(FollowupNote::Current(text))
}

/// Retain the claim in the same transaction as the actual delivery reservation.
/// Ordinary shutdown may refund a wake's attempt, but cannot prove a brief was unsent.
pub(crate) fn capture_for_claim(tx: &mut Tx<'_>, wake: &wakes::WakeRow) -> Result<()> {
    if !is_brief(&wake.source) {
        return Ok(());
    }
    let invalid = || StoreError::Invalid("patrol brief claim identity changed".into());
    let target = decode::<Target>(wake.note.as_deref().ok_or_else(invalid)?)?;
    let mut dispatch = load(tx.conn(), &target.crew)?.ok_or_else(invalid)?;
    if !owns_wake(&dispatch, wake) || dispatch.claimed {
        return Err(invalid().into());
    }
    dispatch.claimed = true;
    meta_set(tx.conn(), &key(&target.crew), &encode(&dispatch)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok_at, store};
    use crewd_proto::WorkCreateParams;
    use crewd_store::Store;

    fn unowned(s: &mut Store, crew: &str, at: i64) -> String {
        ok_at(s, at, |tx| {
            crate::work::create(
                tx,
                &crate::Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: "private work title stays local".into(),
                    ..Default::default()
                },
            )
        })
        .0
        .id
    }

    fn finish(s: &mut Store, at: i64) -> PatrolRun {
        for _ in 0..20 {
            ok_at(s, at, crate::patrol_runtime::tick);
        }
        let run = crate::patrol_runtime::run(s.conn(), "crew-a")
            .unwrap()
            .unwrap();
        assert_eq!(run.completed_at_ms, Some(at));
        run
    }

    fn deliver(s: &mut Store, at: i64, run: &PatrolRun) {
        ok_at(s, at, |tx| {
            enqueue(tx, "crew-a", "cap-a", "test-core:1:1", run)
        });
    }

    fn receipt(s: &Store) -> Option<PatrolCaptainBrief> {
        crate::patrol_runtime::status(s.conn(), "crew-a")
            .unwrap()
            .captain_brief
    }

    fn captain_running(s: &mut Store, at: i64) {
        ok_at(s, at, |tx| {
            crate::sessions::set_desired(tx, "cap-a", Desired::Running)
        });
    }

    #[test]
    fn a_completed_run_delivers_bounded_local_facts_once_even_after_wake_pruning() {
        let mut s = store();
        let local = unowned(&mut s, "crew-a", NOW);
        let foreign = unowned(&mut s, "crew-b", NOW);
        let first = finish(&mut s, NOW);
        deliver(&mut s, NOW, &first);
        assert!(receipt(&s).is_none(), "dormant captain");
        captain_running(&mut s, NOW);
        ok_at(&mut s, NOW, |tx| {
            enqueue(tx, "crew-a", "cap-b", "test-core:1:1", &first)
        });
        assert!(receipt(&s).is_none(), "foreign captain");
        let mut forged = first.clone();
        forged
            .findings
            .insert("unowned".into(), vec![foreign.clone()]);
        deliver(&mut s, NOW, &forged);
        assert!(receipt(&s).is_none(), "foreign reference");
        deliver(&mut s, NOW, &first);
        let queued = receipt(&s).unwrap();
        assert_eq!(queued.phase, PatrolCaptainBriefPhase::Queued);
        let wake = wakes::by_ids(s.conn(), &[queued.wake_id])
            .unwrap()
            .pop()
            .unwrap();
        assert!(matches_generation(
            &wake.source,
            wake.note.as_deref(),
            "test-core:1:1"
        ));
        assert!(!matches_generation(
            &wake.source,
            wake.note.as_deref(),
            "test-core:2:1"
        ));
        let FollowupNote::Current(note) = render(s.conn(), &wake).unwrap() else {
            panic!("missing actual brief")
        };
        assert!(note.contains(&local));
        assert!(!note.contains(&foreign) && !note.contains("private work title"));
        ok_at(&mut s, NOW, |tx| wakes::claim(tx, &[wake.id]));
        assert_eq!(
            receipt(&s).unwrap().phase,
            PatrolCaptainBriefPhase::Inflight
        );
        ok_at(&mut s, NOW, |tx| wakes::mark_delivered(tx, &[wake.id]));
        assert_eq!(
            receipt(&s).unwrap().phase,
            PatrolCaptainBriefPhase::Delivered
        );
        ok_at(&mut s, NOW + 1, |tx| wakes::prune(tx, NOW + 1, 20));
        assert_eq!(receipt(&s).unwrap().phase, PatrolCaptainBriefPhase::Unknown);
        deliver(&mut s, NOW + 1, &first);
        assert_eq!(receipt(&s).unwrap().wake_id, wake.id);
        let unchanged = finish(&mut s, NOW + INTERVAL_MS);
        deliver(&mut s, NOW + INTERVAL_MS, &unchanged);
        assert!(receipt(&s).is_none());
        let second_at = NOW + 2 * INTERVAL_MS;
        unowned(&mut s, "crew-a", second_at);
        let changed = finish(&mut s, second_at);
        deliver(&mut s, second_at, &changed);
        assert_ne!(receipt(&s).unwrap().wake_id, wake.id);
    }

    #[test]
    fn actual_claim_remains_unknown_and_non_replayable_after_attempt_refund() {
        use crate::cursor_reservations as cr;
        let mut s = store();
        unowned(&mut s, "crew-a", NOW);
        let run = finish(&mut s, NOW);
        captain_running(&mut s, NOW);
        deliver(&mut s, NOW, &run);
        let id = receipt(&s).unwrap().wake_id;
        let rows = wakes::by_ids(s.conn(), &[id]).unwrap();
        let mut snapshot = cr::snapshot(s.conn(), "cap-a", &[], NOW).unwrap();
        snapshot
            .capture_work_followups(s.conn(), &rows, NOW)
            .unwrap();
        let cr::Decision::Applied(lease) = ok_at(&mut s, NOW, |tx| {
            cr::claim_reserved(tx, &snapshot, &[], &[id], &[], cr::Owner::new(1)?)
        })
        .0
        else {
            panic!("brief claim was not reserved");
        };
        assert_eq!(
            receipt(&s).unwrap().phase,
            PatrolCaptainBriefPhase::Inflight
        );
        assert!(matches!(
            ok_at(&mut s, NOW, |tx| {
                cr::settle_reserved(tx, &lease, cr::Terminal::Requeue)
            })
            .0,
            cr::Decision::Applied(())
        ));
        let wake = wakes::by_ids(s.conn(), &[id]).unwrap().pop().unwrap();
        assert_eq!((wake.state, wake.attempts), (wakes::WakeState::Pending, 0));
        assert_eq!(receipt(&s).unwrap().phase, PatrolCaptainBriefPhase::Unknown);
        assert!(matches!(
            render(s.conn(), &wake).unwrap(),
            FollowupNote::Stale
        ));
        deliver(&mut s, NOW, &run);
        assert_eq!(receipt(&s).unwrap().wake_id, id);
    }

    #[test]
    fn stop_and_captain_change_fence_render_and_same_run_never_requeues() {
        let mut s = store();
        unowned(&mut s, "crew-a", NOW);
        let run = finish(&mut s, NOW);
        captain_running(&mut s, NOW);
        deliver(&mut s, NOW, &run);
        let id = receipt(&s).unwrap().wake_id;
        let wake = wakes::by_ids(s.conn(), &[id]).unwrap().pop().unwrap();
        ok_at(&mut s, NOW, |tx| wakes::human_stop(tx, "cap-a"));
        assert_eq!(receipt(&s).unwrap().phase, PatrolCaptainBriefPhase::Dropped);
        assert!(matches!(
            render(s.conn(), &wake).unwrap(),
            FollowupNote::Stale
        ));
        captain_running(&mut s, NOW);
        deliver(&mut s, NOW, &run);
        assert_eq!(receipt(&s).unwrap().wake_id, id);
        ok_at(&mut s, NOW, |tx| {
            crate::crews::set_captain(tx, "crew-a", "w-a1")
        });
        assert!(receipt(&s).is_none());
        assert!(matches!(
            render(s.conn(), &wake).unwrap(),
            FollowupNote::Stale
        ));
    }
}
