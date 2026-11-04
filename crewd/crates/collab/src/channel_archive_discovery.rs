//! Idle-channel findings for Human review. Discovery never changes a channel.
use crate::{Result, Sender, channel_archive};
use crewd_proto::{
    WorkPatrolChannel, WorkPatrolChannelBlocker, channel_archive::ChannelArchiveCheckParams,
};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) const IDLE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

pub(crate) enum Review {
    Candidate,
    Ineligible,
    Incomplete,
}

fn idle(at: &str, now_ms: i64) -> Result<bool> {
    let observed = chrono::DateTime::parse_from_rfc3339(at)
        .map_err(|_| {
            crewd_store::StoreError::Invalid("unreadable archive activity timestamp".into())
        })?
        .timestamp_millis();
    Ok(now_ms
        .checked_sub(observed)
        .is_some_and(|age| age >= IDLE_MS))
}

/// Reuse definite cheap blockers, but let the full check resolve terminal #/H
/// references. A requirement can supply actual work without a direct W link.
pub(crate) fn old_enough(c: &Connection, row: &WorkPatrolChannel, now_ms: i64) -> Result<bool> {
    use WorkPatrolChannelBlocker::*;
    if row
        .blocked_by
        .iter()
        .any(|reason| matches!(reason, LiveWork | SourceRemoved | LinksTruncated))
        || (!row.archive_candidate && !row.blocked_by.contains(&NonWorkLinks))
    {
        return Ok(false);
    }
    let last: Option<String> = c
        .query_row(
            "SELECT m.created_at FROM channels ch JOIN messages m ON m.channel_id=ch.id
         WHERE ch.id=?1 AND ch.crew_id=?2 AND ch.kind='topic' AND ch.archived_at IS NULL
         ORDER BY m.seq DESC LIMIT 1",
            params![row.id, row.crew_id],
            |message| message.get(0),
        )
        .optional()?;
    last.map_or(Ok(false), |at| idle(&at, now_ms))
}

/// Use the canonical evidence checks. The result intentionally contains no
/// action revision or grant. An explicit review must read the current facts.
pub(crate) fn verify(c: &Connection, row: &WorkPatrolChannel, now_ms: i64) -> Result<Review> {
    let checked = channel_archive::check(
        c,
        &Sender::Human,
        ChannelArchiveCheckParams {
            channel_id: row.id.clone(),
        },
        now_ms,
    )?;
    if checked.channel.crew_id != row.crew_id {
        return Ok(Review::Ineligible);
    }
    if checked.facts.incomplete {
        return Ok(Review::Incomplete);
    }
    if checked.can_archive
        && let Some(at) = checked.facts.last_message_at
        && idle(&at, now_ms)?
    {
        return Ok(Review::Candidate);
    }
    Ok(Review::Ineligible)
}
