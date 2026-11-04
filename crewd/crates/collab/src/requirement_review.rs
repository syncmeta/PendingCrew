//! Human annotations of exact scored pairs. Original requirements remain separate.
use crate::{
    CollabError, Result, Tx, crews, judgments,
    refusal::{Code, Refusal},
    requirement_similarity::{self as similarity, ReviewSources},
    resource_acl,
};
use crewd_proto::admin_cases::{
    RequirementReview, RequirementReviewDisposition as Disposition, RequirementReviewParams,
    RequirementReviewRow, RequirementReviewsPage, RequirementReviewsParams,
};
use crewd_store::{StoreError, judgment_state, meta_get, meta_set};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "requirement.review.v1:";
const NOTE_BYTES: usize = 4096;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    review: RequirementReview,
    source_witness: String,
    checksum: String,
}

fn invalid() -> CollabError {
    StoreError::Invalid("invalid requirement review record".into()).into()
}

fn conflict() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::RequirementReviewConflict))
}

fn prefix(crew: &str) -> Result<String> {
    Ok(format!("{PREFIX}{}:", judgments::fingerprint(&crew)?))
}

fn pair(first: &str, second: &str) -> Option<(i64, i64)> {
    fn number(id: &str) -> Option<i64> {
        let n: i64 = id.strip_prefix('#')?.parse().ok()?;
        (n > 0 && id == format!("#{n}")).then_some(n)
    }
    let (first, second) = (number(first)?, number(second)?);
    (first < second).then_some((first, second))
}

fn suffix(first: i64, second: i64) -> String {
    format!("{first:020}:{second:020}")
}

fn revision(raw: &str) -> Option<u64> {
    raw.parse::<u64>().ok().filter(|n| n.to_string() == raw)
}

fn hex_digest(raw: &str) -> bool {
    raw.len() == 64
        && raw
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_note(note: &str) -> bool {
    !note.trim().is_empty() && note.len() <= NOTE_BYTES
}

impl Stored {
    fn decode(raw: &str, crew: &str, first: i64, second: i64) -> Result<Self> {
        let record: Self = serde_json::from_str(raw).map_err(StoreError::from)?;
        let review = &record.review;
        if record.version != 1
            || review.crew_id != crew
            || pair(&review.first_id, &review.second_id) != Some((first, second))
            || review.channel_id.is_empty()
            || revision(&review.revision).is_none_or(|n| n == 0)
            || !valid_note(&review.note)
            || !hex_digest(&record.source_witness)
            || !valid_candidate(review.disposition, review.candidate_revision.as_deref())
            || record.checksum != judgments::fingerprint(&(review, &record.source_witness))?
        {
            return Err(invalid());
        }
        Ok(record)
    }

    fn matches_source(&self, witness: &str) -> bool {
        self.review.disposition != Disposition::Clear && self.source_witness == witness
    }
}

fn valid_candidate(disposition: Disposition, candidate: Option<&str>) -> bool {
    match disposition {
        Disposition::Clear => candidate.is_none(),
        Disposition::Group | Disposition::Dismiss => candidate.is_some_and(hex_digest),
    }
}

pub(crate) fn candidate_state(
    c: &Connection,
    crew: &str,
    first: i64,
    second: i64,
    witness: &str,
) -> Result<(String, bool)> {
    let key = format!("{}{}", prefix(crew)?, suffix(first, second));
    let Some(raw) = meta_get(c, &key)? else {
        return Ok(("0".into(), false));
    };
    let record = Stored::decode(&raw, crew, first, second)?;
    let effective = record.matches_source(witness);
    Ok((record.review.revision, effective))
}

/// Only the authenticated Human SQL command port may call this operation.
/// The annotation and exact generic command receipt share the transaction.
pub fn apply(tx: &mut Tx<'_>, p: RequirementReviewParams) -> Result<RequirementReview> {
    let (first, second) =
        pair(&p.first_id, &p.second_id).ok_or_else(|| resource_acl::invalid("pair"))?;
    let expected = revision(&p.expected_review_revision)
        .ok_or_else(|| resource_acl::invalid("expectedReviewRevision"))?;
    let next = expected
        .checked_add(1)
        .ok_or_else(|| resource_acl::invalid("expectedReviewRevision"))?;
    if !valid_note(&p.note) {
        return Err(resource_acl::invalid("note"));
    }
    if !valid_candidate(p.disposition, p.candidate_revision.as_deref()) {
        return Err(resource_acl::invalid("candidateRevision"));
    }
    let c = tx.conn();
    crews::refuse_archived(c, &p.crew_id)?;
    let key = format!("{}{}", prefix(&p.crew_id)?, suffix(first, second));
    let old = meta_get(c, &key)?
        .map(|raw| Stored::decode(&raw, &p.crew_id, first, second))
        .transpose()?;
    let previous_revision = old.as_ref().map_or("0", |row| row.review.revision.as_str());
    if previous_revision != p.expected_review_revision {
        return Err(conflict());
    }
    let (channel_id, source_witness) = if p.disposition == Disposition::Clear {
        let old = old
            .filter(|row| row.review.disposition != Disposition::Clear)
            .ok_or_else(conflict)?;
        (old.review.channel_id, old.source_witness)
    } else {
        let candidate =
            similarity::current_suggestion(c, &p.crew_id, first, second)?.ok_or_else(conflict)?;
        if p.candidate_revision.as_deref() != Some(candidate.candidate_revision()?.as_str())
            || old.is_some_and(|row| row.matches_source(&candidate.review_witness))
        {
            return Err(conflict());
        }
        (candidate.channel, candidate.review_witness)
    };
    let review = RequirementReview {
        crew_id: p.crew_id,
        first_id: p.first_id,
        second_id: p.second_id,
        channel_id,
        revision: next.to_string(),
        disposition: p.disposition,
        note: p.note,
        changed_at_ms: tx.now_ms(),
        candidate_revision: p.candidate_revision,
    };
    let record = Stored {
        version: 1,
        checksum: judgments::fingerprint(&(&review, &source_witness))?,
        review,
        source_witness,
    };
    meta_set(
        c,
        &key,
        &serde_json::to_string(&record).map_err(StoreError::from)?,
    )?;
    Ok(record.review)
}

fn parse_suffix(raw: &str) -> Option<(i64, i64)> {
    let (first, second) = raw.split_once(':')?;
    let (first, second) = (first.parse::<i64>().ok()?, second.parse::<i64>().ok()?);
    (first > 0 && first < second && suffix(first, second) == raw).then_some((first, second))
}

pub fn list(c: &Connection, p: &RequirementReviewsParams) -> Result<RequirementReviewsPage> {
    let archived = crews::is_archived(c, &p.crew_id)?;
    let limit = usize::from(p.limit.unwrap_or(20));
    if !(1..=50).contains(&limit) {
        return Err(resource_acl::invalid("limit"));
    }
    let prefix = prefix(&p.crew_id)?;
    let scope = judgments::fingerprint(&(judgment_state::epoch(c)?, &p.crew_id))?;
    let after = match p.after.as_deref() {
        None => "",
        Some(raw) => {
            let (epoch, pair) = raw
                .split_once(':')
                .ok_or_else(|| resource_acl::invalid("after"))?;
            if epoch != scope || parse_suffix(pair).is_none() {
                return Err(resource_acl::invalid("after"));
            }
            pair
        }
    };
    let mut stmt =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let rows = stmt
        .query_map(
            params![
                format!("{prefix}{after}"),
                format!("{prefix}\u{10ffff}"),
                (limit + 1) as i64
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > limit;
    let authority = (!archived && !rows.is_empty())
        .then(|| ReviewSources::load(c))
        .transpose()?;
    let mut page = RequirementReviewsPage {
        crew_id: p.crew_id.clone(),
        items: Vec::new(),
        next_after: None,
    };
    for (key, raw) in rows.into_iter().take(limit) {
        let pair = key.strip_prefix(&prefix).ok_or_else(invalid)?;
        let (first, second) = parse_suffix(pair).ok_or_else(invalid)?;
        let record = Stored::decode(&raw, &p.crew_id, first, second)?;
        let effective = if let Some(authority) = &authority
            && record.review.disposition != Disposition::Clear
        {
            authority
                .witness(c, &p.crew_id, &record.review.channel_id, first, second)?
                .is_some_and(|witness| record.matches_source(&witness))
        } else {
            false
        };
        page.items.push(RequirementReviewRow {
            review: record.review,
            effective,
        });
        if more {
            page.next_after = Some(format!("{scope}:{pair}"));
        }
    }
    Ok(page)
}
