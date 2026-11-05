//! Core-managed D decisions. The original draft, actor and birth never change.
use crate::{Result, StoreError, managed_objects, object_refs::ObjectKind};
use crewd_proto::{
    ResourceActor,
    managed_objects::{Decision, DecisionAbout, DecisionDraft, DecisionStatus, DecisionWithdrawal},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

pub const DDL: &str = r#"
CREATE TABLE managed_decisions (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    draft_json TEXT NOT NULL CHECK(json_valid(draft_json) AND json_type(draft_json)='object'),
    author_json TEXT NOT NULL CHECK(json_valid(author_json) AND json_type(author_json)='object'),
    status TEXT NOT NULL CHECK(status IN ('active','superseded','withdrawn')),
    supersedes INTEGER UNIQUE REFERENCES managed_decisions(number) DEFERRABLE INITIALLY DEFERRED,
    withdrawal_json TEXT CHECK(withdrawal_json IS NULL OR (json_valid(withdrawal_json) AND json_type(withdrawal_json)='object')),
    CHECK(supersedes IS NULL OR supersedes < number),
    CHECK((status='withdrawn') = (withdrawal_json IS NOT NULL))
);
"#;

fn invalid(reason: &str) -> StoreError {
    StoreError::Invalid(reason.into())
}

pub fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().take(limit + 1).count() <= limit
        && !value.contains('\0')
}

pub fn about_key(reference: &DecisionAbout) -> (&'static str, &str) {
    match reference {
        DecisionAbout::Crew { id } => ("crew", id),
        DecisionAbout::Work { id } => ("work", id),
        DecisionAbout::Requirement { id } => ("requirement", id),
        DecisionAbout::Row { id } => ("row", id),
        DecisionAbout::Risk { id } => ("risk", id),
    }
}

pub fn draft_valid(draft: &DecisionDraft) -> bool {
    valid_text(&draft.question, 60)
        && valid_text(&draft.choice, 1500)
        && valid_text(&draft.rationale, 1500)
        && draft
            .alternatives
            .as_deref()
            .is_none_or(|text| valid_text(text, 1500))
        && (1..=8).contains(&draft.about.len())
        && draft.about.iter().map(about_key).all(|(_, id)| {
            !id.is_empty()
                && id.len() <= 128
                && id.trim() == id
                && !id.chars().any(char::is_control)
        })
        && draft
            .about
            .iter()
            .map(about_key)
            .collect::<HashSet<_>>()
            .len()
            == draft.about.len()
}

fn actor_valid(actor: &ResourceActor) -> bool {
    match (actor.kind.as_str(), actor.session_id.as_deref()) {
        ("human", None) => true,
        ("session", Some(id)) => {
            !id.is_empty() && id.len() <= 500 && !id.chars().any(char::is_control)
        }
        _ => false,
    }
}

pub fn get(c: &Connection, number: i64) -> Result<Option<Decision>> {
    type Entry = (
        String,
        String,
        String,
        i64,
        String,
        String,
        String,
        bool,
        Option<i64>,
        Option<i64>,
        Option<String>,
        bool,
    );
    let record: Option<Entry> = c.query_row(
        "SELECT r.crew_id,r.source_id,r.created_at,d.revision,d.draft_json,d.author_json,d.status,
                c.archived_at IS NOT NULL,d.supersedes,next.number,d.withdrawal_json,
                (d.supersedes IS NULL OR (previous.status='superseded' AND parent.crew_id=r.crew_id))
                AND (next.number IS NULL OR successor.crew_id=r.crew_id)
         FROM managed_decisions d JOIN object_refs r ON d.number=r.number AND r.kind='decision'
         JOIN crews c ON c.id=r.crew_id
         LEFT JOIN managed_decisions previous ON previous.number=d.supersedes
         LEFT JOIN object_refs parent ON parent.number=previous.number AND parent.kind='decision'
         LEFT JOIN managed_decisions next ON next.supersedes=d.number
         LEFT JOIN object_refs successor ON successor.number=next.number AND successor.kind='decision'
         WHERE d.number=?1",
        [number],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get::<_,Option<bool>>(11)?.unwrap_or(false))),
    ).optional()?;
    record
        .map(
            |(
                crew_id,
                source,
                created_at,
                revision,
                draft,
                author,
                status,
                frozen,
                supersedes,
                superseded_by,
                withdrawal,
                chain_valid,
            )| {
                let status = match (
                    status.as_str(),
                    revision,
                    superseded_by.is_some(),
                    withdrawal.is_some(),
                ) {
                    ("active", 1, false, false) => DecisionStatus::Active,
                    ("superseded", 2, true, false) => DecisionStatus::Superseded,
                    ("withdrawn", 2, false, true) => DecisionStatus::Withdrawn,
                    _ => return Err(invalid("managed decision lifecycle is invalid")),
                };
                let draft = serde_json::from_str::<DecisionDraft>(&draft)?;
                let author = serde_json::from_str::<ResourceActor>(&author)?;
                let withdrawal = withdrawal
                    .as_deref()
                    .map(serde_json::from_str::<DecisionWithdrawal>)
                    .transpose()?;
                if !managed_objects::source(ObjectKind::Decision, &source)
                    || !chain_valid
                    || created_at.is_empty()
                    || !draft_valid(&draft)
                    || !actor_valid(&author)
                    || withdrawal.as_ref().is_some_and(|w| {
                        !actor_valid(&w.author) || !valid_text(&w.reason, 1500) || w.at.is_empty()
                    })
                {
                    return Err(invalid("managed decision evidence is invalid"));
                }
                Ok(Decision {
                    id: format!("D{number}"),
                    crew_id,
                    revision: revision.to_string(),
                    draft,
                    author,
                    created_at,
                    status,
                    frozen,
                    supersedes: supersedes.map(|n| format!("D{n}")),
                    superseded_by: superseded_by.map(|n| format!("D{n}")),
                    withdrawal,
                })
            },
        )
        .transpose()
}

pub fn numbers(c: &Connection, after: Option<i64>, limit: u16) -> Result<Vec<i64>> {
    let mut q = c.prepare("SELECT number FROM managed_decisions WHERE ?1 IS NULL OR number<?1 ORDER BY number DESC LIMIT ?2")?;
    Ok(q.query_map(params![after, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn record(
    c: &Connection,
    crew: &str,
    draft: &DecisionDraft,
    author: &ResourceActor,
    at: &str,
    supersedes: Option<i64>,
) -> Result<Decision> {
    managed_objects::transaction(c)?;
    let reference = managed_objects::allocate(c, ObjectKind::Decision, crew, at)?;
    c.execute(
        "INSERT INTO managed_decisions(number,revision,draft_json,author_json,status,supersedes)
         VALUES(?1,1,?2,?3,'active',?4)",
        params![
            reference.number,
            serde_json::to_string(draft)?,
            serde_json::to_string(author)?,
            supersedes
        ],
    )?;
    get(c, reference.number)?.ok_or_else(|| invalid("committed decision source missing"))
}

pub fn supersede(c: &Connection, number: i64, revision: i64) -> Result<()> {
    managed_objects::transaction(c)?;
    let changed = c.execute(
        "UPDATE managed_decisions SET revision=revision+1,status='superseded'
         WHERE number=?1 AND revision=?2 AND status='active'",
        params![number, revision],
    )?;
    if changed != 1 {
        return Err(invalid("managed decision changed within transaction"));
    }
    Ok(())
}

pub fn withdraw(
    c: &Connection,
    number: i64,
    revision: i64,
    withdrawal: &DecisionWithdrawal,
) -> Result<Decision> {
    managed_objects::transaction(c)?;
    let changed = c.execute(
        "UPDATE managed_decisions SET revision=revision+1,status='withdrawn',withdrawal_json=?3
         WHERE number=?1 AND revision=?2 AND status='active'",
        params![number, revision, serde_json::to_string(withdrawal)?],
    )?;
    if changed != 1 {
        return Err(invalid("managed decision changed within transaction"));
    }
    get(c, number)?.ok_or_else(|| invalid("committed withdrawal source missing"))
}
