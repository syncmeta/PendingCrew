//! Explicit decision questions anchored to actual H identities and terminal WorkBook events.
use crate::{Result, StoreError, managed_decisions, managed_objects};
use crewd_proto::{
    decision_asks::{Capture, Draft, OpenReceipt, Source, View},
    managed_objects::DecisionDraft,
};
use crewd_work::{Actor, Event, EventKind};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

pub const DDL: &str = r#"
CREATE TABLE decision_asks (
    ask_number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    draft_json TEXT NOT NULL CHECK(json_valid(draft_json) AND json_type(draft_json)='object'),
    decision_number INTEGER UNIQUE REFERENCES managed_decisions(number) ON DELETE CASCADE,
    event_seq INTEGER UNIQUE REFERENCES organization_work_events(seq) DEFERRABLE INITIALLY DEFERRED,
    option_id TEXT CHECK(option_id IN ('A','B','C','D','E','F','G','H')),
    source TEXT CHECK(source IN ('human','systemDefault')),
    human_reason TEXT,
    CHECK((decision_number IS NULL)=(event_seq IS NULL)),
    CHECK((decision_number IS NULL)=(option_id IS NULL)),
    CHECK((decision_number IS NULL)=(source IS NULL)),
    CHECK(human_reason IS NULL OR (source IS NOT NULL AND source='human')),
    CHECK(decision_number IS NULL OR ask_number<decision_number)
);
CREATE TABLE decision_ask_open_receipts (
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    ask_number INTEGER NOT NULL UNIQUE REFERENCES decision_asks(ask_number) ON DELETE CASCADE,
    digest BLOB NOT NULL CHECK(length(digest)=32),
    receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json) AND json_type(receipt_json)='object'),
    PRIMARY KEY(crew_id,session_id,request_id)
) WITHOUT ROWID;
CREATE TABLE public_command_purge_tombstones (
    principal TEXT NOT NULL CHECK(length(principal) BETWEEN 1 AND 512),
    request_id TEXT NOT NULL,
    db_id TEXT NOT NULL,
    PRIMARY KEY(principal,request_id,db_id)
);
"#;

const IDS: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];

fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}

pub fn option_index(draft: &Draft, id: &str) -> Option<usize> {
    IDS.iter()
        .position(|known| *known == id)
        .filter(|i| *i < draft.options.len())
}

pub fn draft_valid(draft: &Draft) -> bool {
    use managed_decisions::valid_text;
    valid_text(&draft.question, 60)
        && draft.question.trim() == draft.question
        && valid_text(&draft.context, 900)
        && (2..=IDS.len()).contains(&draft.options.len())
        && draft.options.iter().all(|option| {
            valid_text(&option.label, 120)
                && option.label.trim() == option.label
                && !option
                    .label
                    .chars()
                    .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
        })
        && draft
            .options
            .iter()
            .map(|option| &option.label)
            .collect::<HashSet<_>>()
            .len()
            == draft.options.len()
        && draft.default.as_ref().is_none_or(|choice| {
            (1..=10080).contains(&choice.after_minutes)
                && option_index(draft, &choice.option_id).is_some()
        })
        && managed_decisions::draft_valid(&DecisionDraft {
            question: draft.question.clone(),
            choice: draft.question.clone(),
            rationale: draft.context.clone(),
            alternatives: None,
            reversibility: draft.reversibility,
            about: draft.about.clone(),
        })
}

/// The WorkBook keeps the trimmed original Human text; derive its exact option
/// and separately bounded reason without interpreting prose or a recommendation.
pub fn parse_human<'a>(draft: &Draft, text: &'a str) -> Option<(&'a str, Option<&'a str>)> {
    let text = text.trim();
    let (option, reason) = match text.split_once('\n') {
        Some((id, reason)) if managed_decisions::valid_text(reason, 500) => (id, Some(reason)),
        Some(_) => return None,
        None => (text, None),
    };
    option_index(draft, option).map(|_| (option, reason))
}

pub fn decision_draft(draft: &Draft, option: &str) -> Result<DecisionDraft> {
    let selected =
        option_index(draft, option).ok_or_else(|| invalid("invalid captured decision option"))?;
    let other = draft
        .options
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != selected)
        .map(|(i, option)| format!("{} {}", IDS[i], option.label))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(DecisionDraft {
        question: draft.question.clone(),
        choice: draft.options[selected].label.clone(),
        rationale: draft.context.clone(),
        alternatives: Some(other),
        reversibility: draft.reversibility,
        about: draft.about.clone(),
    })
}

pub fn announcement(draft: &Draft) -> String {
    let choices = draft
        .options
        .iter()
        .zip(IDS)
        .map(|(option, id)| format!("{id} {}", option.label))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n\n{}\n\n{}", draft.question, draft.context, choices)
}

pub fn read_ask(c: &Connection, number: i64) -> Result<Option<View>> {
    read(c, number, false)
}

pub fn read_decision(c: &Connection, number: i64) -> Result<Option<View>> {
    read(c, number, true)
}

fn read(c: &Connection, number: i64, by_decision: bool) -> Result<Option<View>> {
    let predicate = if by_decision {
        "a.decision_number=?1"
    } else {
        "a.ask_number=?1"
    };
    let sql = format!(
        "SELECT a.ask_number,a.crew_id,a.draft_json,a.decision_number,a.event_seq,a.option_id,
                a.source,a.human_reason,h.core_id,e.crew_id,e.body,d.crew_id,d.created_at
         FROM decision_asks a
         LEFT JOIN object_refs h ON h.number=a.ask_number AND h.kind='human_task' AND h.crew_id=a.crew_id
         LEFT JOIN organization_work_events e ON e.seq=a.event_seq
         LEFT JOIN object_refs d ON d.number=a.decision_number AND d.kind='decision'
         WHERE {predicate}"
    );
    let entry = c
        .query_row(&sql, [number], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
            ))
        })
        .optional()?;
    entry
        .map(
            |(
                ask,
                crew_id,
                draft,
                d,
                seq,
                option,
                source,
                reason,
                core,
                event_crew,
                raw_event,
                decision_crew,
                decision_at,
            )| {
                let draft: Draft = serde_json::from_str(&draft)?;
                if !draft_valid(&draft) {
                    return Err(invalid("invalid stored decision question"));
                }
                let core = core
                    .and_then(|id| {
                        id.parse::<crewd_work::AskId>()
                            .ok()
                            .filter(|core| core.to_string() == id)
                    })
                    .ok_or_else(|| invalid("missing exact decision question authority"))?;
                let capture = match (d, seq, option, source, raw_event) {
                    (None, None, None, None, None) if reason.is_none() => None,
                    (Some(d), Some(seq), Some(option_id), Some(source), Some(raw))
                        if d > ask
                            && seq > 0
                            && event_crew.as_ref() == Some(&crew_id)
                            && decision_crew.as_ref() == Some(&crew_id) =>
                    {
                        let event: Event = serde_json::from_str(&raw)?;
                        let (actual_option, actual_reason, actual_source) =
                            terminal_choice(&draft, core, &event).ok_or_else(|| {
                                invalid("decision question event does not prove capture")
                            })?;
                        let source = match source.as_str() {
                            "human" => Source::Human,
                            "systemDefault" => Source::SystemDefault,
                            _ => return Err(invalid("invalid captured decision source")),
                        };
                        if event.seq != seq as u64
                            || decision_at.as_deref()
                                != Some(crate::ts_from_ms(event.at.0).as_str())
                            || actual_option != option_id
                            || actual_reason != reason.as_deref()
                            || actual_source != source
                        {
                            return Err(invalid("decision question capture mismatch"));
                        }
                        Some(Capture {
                            decision_id: format!("D{d}"),
                            event_seq: seq.to_string(),
                            option_id,
                            source,
                            human_reason: reason,
                        })
                    }
                    _ => return Err(invalid("invalid decision question relation")),
                };
                Ok(View {
                    crew_id,
                    ask: format!("H{ask}"),
                    draft,
                    capture,
                })
            },
        )
        .transpose()
}

pub fn terminal_choice<'a>(
    draft: &'a Draft,
    ask: crewd_work::AskId,
    event: &'a Event,
) -> Option<(&'a str, Option<&'a str>, Source)> {
    match (&event.by, &event.kind) {
        (Actor::Human, EventKind::AskAnswered { ask: actual, text }) if *actual == ask => {
            let (option, reason) = parse_human(draft, text)?;
            Some((option, reason, Source::Human))
        }
        (Actor::System, EventKind::AskDefaulted { ask: actual, text }) if *actual == ask => {
            let default = draft.default.as_ref()?;
            (text == &default.option_id).then_some((text.as_str(), None, Source::SystemDefault))
        }
        _ => None,
    }
}

pub fn insert(c: &Connection, number: i64, crew: &str, draft: &Draft) -> Result<View> {
    managed_objects::transaction(c)?;
    c.execute(
        "INSERT INTO decision_asks(ask_number,crew_id,draft_json) VALUES (?1,?2,?3)",
        params![number, crew, serde_json::to_string(draft)?],
    )?;
    read_ask(c, number)?.ok_or_else(|| invalid("inserted decision question missing"))
}

pub(crate) fn complete(
    c: &Connection,
    ask_number: i64,
    decision_number: i64,
    event_seq: u64,
    option: &str,
    source: Source,
    reason: Option<&str>,
) -> Result<()> {
    managed_objects::transaction(c)?;
    let event_seq =
        i64::try_from(event_seq).map_err(|_| invalid("decision question event exhausted"))?;
    let source = match source {
        Source::Human => "human",
        Source::SystemDefault => "systemDefault",
    };
    let changed = c.execute(
        "UPDATE decision_asks SET decision_number=?2,event_seq=?3,option_id=?4,source=?5,human_reason=?6
         WHERE ask_number=?1 AND decision_number IS NULL",
        params![ask_number, decision_number, event_seq, option, source, reason],
    )?;
    if changed != 1 {
        return Err(invalid("decision question already captured or missing"));
    }
    Ok(())
}

pub fn receipt(
    c: &Connection,
    crew: &str,
    session: &str,
    request_id: &str,
) -> Result<Option<([u8; 32], OpenReceipt)>> {
    let entry = c
        .query_row(
            "SELECT digest,receipt_json,ask_number FROM decision_ask_open_receipts
         WHERE crew_id=?1 AND session_id=?2 AND request_id=?3",
            params![crew, session, request_id],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    entry
        .map(|(digest, json, ask_number)| {
            let receipt: OpenReceipt = serde_json::from_str(&json)?;
            let current = read_ask(c, ask_number)?
                .ok_or_else(|| invalid("decision question receipt has no source"))?;
            if receipt.request_id != request_id
                || receipt.opened.crew_id != crew
                || receipt.opened.crew_id != current.crew_id
                || receipt.opened.ask != current.ask
                || receipt.opened.draft != current.draft
                || receipt.opened.capture.is_some()
            {
                return Err(invalid(
                    "decision question receipt scope or content mismatch",
                ));
            }
            let digest = digest
                .try_into()
                .map_err(|_| invalid("decision question receipt digest mismatch"))?;
            Ok((digest, receipt))
        })
        .transpose()
}

pub fn record(
    c: &Connection,
    ask_number: i64,
    session: &str,
    digest: &[u8],
    receipt: &OpenReceipt,
) -> Result<()> {
    managed_objects::transaction(c)?;
    c.execute(
        "INSERT INTO decision_ask_open_receipts(crew_id,session_id,request_id,ask_number,digest,receipt_json)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![receipt.opened.crew_id, session, receipt.request_id, ask_number, digest, serde_json::to_string(receipt)?],
    )?;
    Ok(())
}
