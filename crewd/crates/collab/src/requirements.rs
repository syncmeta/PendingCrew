//! Human source facts plus derived organization state; no secondary lifecycle authority.
use crate::refusal::{Code, Refusal};
use crate::wakes::{self, WakeKind, WakePriority, WakeRequest};
use crate::{Change, CollabError, Result, Sender, Tx, asks, crews, sessions};
use crewd_proto::{
    ChannelWorkKind, ChannelWorkObject, Ledger, RequirementClosure, RequirementDetail, Todo,
    TodoCloseParams, TodoCreateLookupParams, TodoCreateLookupReceipt, TodoCreateLookupResult,
    TodoCreateParams,
};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_work::{Actor, EntryNo};
use rusqlite::{Connection, OptionalExtension, params};
const SOURCE: &str = "requirement-new:";
fn invalid(code: Code) -> CollabError {
    CollabError::Invalid(Refusal::new(code))
}
fn human(by: &Sender) -> Result<()> {
    if matches!(by, Sender::Human) {
        Ok(())
    } else {
        Err(CollabError::Forbidden(Refusal::new(Code::TodoHumanOnly)))
    }
}
pub(crate) fn reference(conn: &Connection, id: &str) -> Result<ObjectReference> {
    let n = id
        .strip_prefix('#')
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0 && id == format!("#{n}"))
        .ok_or_else(|| invalid(Code::TodoIdInvalid))?;
    object_refs::get(conn, ObjectKind::Requirement, n)?.ok_or_else(|| {
        CollabError::NotFound(Refusal::new(Code::TodoNotFound).arg("number", n.to_string()))
    })
}
pub(crate) fn core(conn: &Connection, r: &ObjectReference) -> Result<EntryNo> {
    crewd_store::work_organization::core_for_reference(conn, ObjectKind::Requirement, r.number)?
        .ok_or_else(|| StoreError::Invalid("requirement lacks organization identity".into()))?
        .parse()
        .map_err(|_| {
            StoreError::Invalid("requirement has invalid organization identity".into()).into()
        })
}
pub fn get(conn: &Connection, id: &str) -> Result<RequirementDetail> {
    let r = reference(conn, id)?;
    detail(conn, &r, &asks::load_org(conn)?)
}
pub(crate) fn detail(
    conn: &Connection,
    r: &ObjectReference,
    book: &crewd_work::WorkBook,
) -> Result<RequirementDetail> {
    let raw: Option<String> = conn.query_row(
        "SELECT raw_todo_id FROM object_refs WHERE number=?1",
        [r.number],
        |row| row.get(0),
    )?;
    let raw = raw.ok_or_else(|| StoreError::Invalid("requirement lacks original source".into()))?;
    let mut todo = crewd_store::todos::get_raw(conn, &raw)?
        .ok_or_else(|| StoreError::Invalid("requirement original source is missing".into()))?;
    if todo.ledger != Ledger::Agent || todo.crew_id != r.crew_id {
        return Err(
            StoreError::Invalid("requirement original source identity mismatch".into()).into(),
        );
    }
    let no = core(conn, r)?;
    let view = book
        .entry_view(no)
        .ok_or_else(|| StoreError::Invalid("requirement lacks authoritative entry".into()))?;
    todo.requirement_id = Some(r.id());
    let derived_status = crewd_store::todos::requirement_status(view.status, None).to_string();
    let todo_attributes = crate::todo_attributes::get(
        conn,
        &Sender::Human,
        &crewd_proto::ChannelWorkObject {
            kind: crewd_proto::ChannelWorkKind::Requirement,
            id: r.id(),
        },
    )?;
    todo.status =
        crewd_store::todos::requirement_status(view.status, todo_attributes.manual_state).into();
    let closure = book.state().entries[&no]
        .closed
        .as_ref()
        .map(|c| RequirementClosure {
            note: c.note.clone(),
            at: crewd_store::ts_from_ms(c.at.0),
        });
    Ok(RequirementDetail {
        todo_attributes,
        derived_status,
        todo,
        closure,
        channel_association: None,
    })
}
fn original_text_sha256(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Recover only the original immutable request and current source-channel association.
pub fn create_lookup(
    c: &Connection,
    by: &Sender,
    p: TodoCreateLookupParams,
) -> Result<TodoCreateLookupResult> {
    human(by)?;
    let request_id = uuid::Uuid::parse_str(&p.request_id)
        .map_err(|_| invalid(Code::TodoRequestInvalid))?
        .to_string();
    let scope_id = |value: &str| {
        !value.is_empty()
            && value.len() <= 512
            && value.trim() == value
            && !value.chars().any(char::is_control)
    };
    if !scope_id(&p.crew_id) {
        return Err(crate::resource_acl::invalid("crewId"));
    }
    if p.channel_id
        .as_deref()
        .is_some_and(|channel| !scope_id(channel))
    {
        return Err(crate::resource_acl::invalid("channelId"));
    }
    if p.text_sha256.len() != 64
        || !p
            .text_sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(crate::resource_acl::invalid("textSha256"));
    }
    let absent = || TodoCreateLookupResult::NotFound {
        request_id: request_id.clone(),
    };
    let raw_id = format!("requirement-request-{request_id}");
    // One indexed original source lookup; do not read mutable response/body projections.
    type OriginalRequirementRow = (String, String, Option<String>, i64, Option<String>);
    let raw: Option<OriginalRequirementRow> = c
        .query_row(
            "SELECT t.text,t.created_at,t.extra,r.number,r.core_id
         FROM todos t
         JOIN object_refs r ON r.raw_todo_id=t.id AND r.kind='requirement'
             AND r.crew_id=t.crew_id AND r.source_id='todo:'||t.id AND r.created_at=t.created_at
         WHERE t.id=?1 AND t.crew_id=?2 AND t.ledger='agent' AND t.deleted_at IS NULL",
            params![raw_id, p.crew_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((text, created_at, marker, number, core_id)) = raw else {
        return Ok(absent());
    };
    let marker = marker
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let exact_marker = marker.as_ref().is_some_and(|m| {
        m.get("requirementRequestId")
            .and_then(serde_json::Value::as_str)
            == Some(request_id.as_str())
            && match (m.get("sourceChannelId"), p.channel_id.as_deref()) {
                // The first todo.create marker predates channel creation and omitted this key.
                (None | Some(serde_json::Value::Null), None) => true,
                (Some(serde_json::Value::String(original)), Some(requested)) => {
                    original == requested
                }
                _ => false,
            }
    });
    if !exact_marker || original_text_sha256(&text) != p.text_sha256 || number <= 0 {
        return Ok(absent());
    }
    let Some(created_ms) = chrono::DateTime::parse_from_rfc3339(&created_at)
        .ok()
        .map(|time| time.timestamp_millis())
        .filter(|ms| crewd_store::ts_from_ms(*ms) == created_at)
    else {
        return Ok(absent());
    };
    let Some(entry) = core_id.as_deref().and_then(|id| {
        id.parse::<EntryNo>()
            .ok()
            .filter(|entry| entry.0 > 0 && entry.to_string() == id)
    }) else {
        return Ok(absent());
    };
    let book = crewd_store::work_organization::load(c)?
        .ok_or_else(|| StoreError::Invalid("organization work authority is missing".into()))?;
    // WorkBook clamps registration to its monotonic clock. The original raw Todo
    // and its reference agree on the canonical wall time even if that clock was ahead.
    if book
        .entry_crew(entry)
        .is_none_or(|crew| crew.as_str() != p.crew_id)
        || book
            .entries
            .get(&entry)
            .is_none_or(|entry| entry.registered_at.0 < created_ms)
    {
        return Ok(absent());
    }
    let requirement_id = format!("#{number}");
    let current_association = if let Some(channel) = &p.channel_id {
        match crate::channel_work::association(
            c,
            by,
            channel,
            &ChannelWorkObject {
                kind: ChannelWorkKind::Requirement,
                id: requirement_id.clone(),
            },
        ) {
            Ok(association) => Some(association),
            Err(CollabError::NotFound(_)) => return Ok(absent()),
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    Ok(TodoCreateLookupResult::Committed {
        request_id,
        receipt: TodoCreateLookupReceipt {
            crew_id: p.crew_id,
            requirement_id,
            text_sha256: p.text_sha256,
            created_at,
            source_channel_id: p.channel_id,
            current_association,
        },
    })
}

pub fn create(tx: &mut Tx<'_>, by: &Sender, p: TodoCreateParams) -> Result<RequirementDetail> {
    human(by)?;
    let uuid =
        uuid::Uuid::parse_str(&p.request_id).map_err(|_| invalid(Code::TodoRequestInvalid))?;
    let request = uuid.to_string();
    let raw_id = format!("requirement-request-{request}");
    if let Some(old) = crewd_store::todos::get_raw(tx.conn(), &raw_id)? {
        let extra: Option<String> =
            tx.conn()
                .query_row("SELECT extra FROM todos WHERE id=?1", [&raw_id], |r| {
                    r.get(0)
                })?;
        let marker = extra
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        if old.ledger != Ledger::Agent
            || old.crew_id != p.crew_id
            || old.text != p.text
            || marker
                .as_ref()
                .and_then(|m| m["requirementRequestId"].as_str())
                != Some(request.as_str())
            || marker.as_ref().and_then(|m| m["sourceChannelId"].as_str())
                != p.channel_id.as_deref()
        {
            return Err(CollabError::Conflict(
                Refusal::new(Code::TodoRequestConflict).arg("request", request),
            ));
        }
        let id: String = tx.conn().query_row(
            "SELECT '#'||number FROM object_refs WHERE raw_todo_id=?1 AND kind='requirement'",
            [&raw_id],
            |r| r.get(0),
        )?;
        let mut detail = get(tx.conn(), &id)?;
        if let Some(ch) = &p.channel_id {
            detail.channel_association = Some(crate::channel_work::association(
                tx.conn(),
                by,
                ch,
                &crewd_proto::ChannelWorkObject {
                    kind: crewd_proto::ChannelWorkKind::Requirement,
                    id,
                },
            )?);
        }
        return Ok(detail);
    }
    if let Some(ch) = &p.channel_id {
        crate::channel_work::authorize(tx.conn(), by, ch, true)?;
    }
    if p.text.trim().is_empty() {
        return Err(invalid(Code::TodoTextEmpty));
    }
    crews::get(tx.conn(), &p.crew_id)?;
    crews::refuse_archived(tx.conn(), &p.crew_id)?;
    let local: i64 = tx.conn().query_row(
        "SELECT COALESCE(MAX(number),0)+1 FROM todos WHERE crew_id=?1 AND ledger='agent'",
        [&p.crew_id],
        |r| r.get(0),
    )?;
    let now = tx.now_ts();
    let t = Todo {
        requirement_id: None,
        id: raw_id.clone(),
        crew_id: p.crew_id.clone(),
        ledger: Ledger::Agent,
        number: local,
        text: p.text,
        status: "pending".into(),
        created_at: now,
        updated_at: None,
        deleted_at: None,
        attachments: vec![],
        responses: vec![],
    };
    crewd_store::todos::insert(
        tx.conn(),
        &t,
        Some(&serde_json::json!({"requirementRequestId":request,"sourceChannelId":p.channel_id})),
    )?;
    let id: String = tx.conn().query_row(
        "SELECT '#'||number FROM object_refs WHERE raw_todo_id=?1 AND kind='requirement'",
        [&raw_id],
        |r| r.get(0),
    )?;
    tx.note(Change::WorkChanged {
        crew_id: p.crew_id.clone(),
        id: id.clone(),
    });
    let judgment_reserved = crate::judgments::reserve(
        tx,
        crewd_proto::judgments::JudgmentSourceKind::Requirement,
        &id,
    )?;
    if let Some(captain) = crews::captain_of(tx.conn(), &p.crew_id)?
        && !judgment_reserved
    {
        let session = sessions::get(tx.conn(), &captain)?;
        if session.crew_id != p.crew_id {
            return Err(
                StoreError::Invalid("requirement captain belongs to another crew".into()).into(),
            );
        }
        let n = Notification {
            requirement_id: id.clone(),
            captain: captain.clone(),
            stop_floor: wakes::human_stop_floor(tx.conn(), &captain)?,
        };
        wakes::enqueue(
            tx,
            WakeRequest {
                session: captain,
                kind: WakeKind::System,
                source: format!("{SOURCE}{id}"),
                priority: WakePriority::Human,
                not_before_ms: None,
                note: Some(serde_json::to_string(&n).map_err(crewd_store::StoreError::from)?),
                origin: None,
            },
        )?;
    }
    let mut detail = get(tx.conn(), &id)?;
    if let Some(ch) = &p.channel_id {
        detail.channel_association = Some(crate::channel_work::link_created(
            tx,
            by,
            ch,
            crewd_proto::ChannelWorkObject {
                kind: crewd_proto::ChannelWorkKind::Requirement,
                id,
            },
        )?);
    }
    Ok(detail)
}
pub fn close(tx: &mut Tx<'_>, by: &Sender, p: TodoCloseParams) -> Result<RequirementDetail> {
    human(by)?;
    if !["unclaimed", "in_progress", "done", "needs_ruling", "closed"]
        .contains(&p.expected_status.as_str())
    {
        return Err(invalid(Code::TodoStatusInvalid));
    }
    let current = get(tx.conn(), &p.id)?;
    crews::refuse_archived(tx.conn(), &current.todo.crew_id)?;
    if current.todo.status != p.expected_status {
        return Err(CollabError::Conflict(
            Refusal::new(Code::TodoStatusConflict)
                .arg("id", &p.id)
                .arg("expected", p.expected_status)
                .arg("actual", current.todo.status),
        ));
    }
    let r = reference(tx.conn(), &p.id)?;
    let no = core(tx.conn(), &r)?;
    asks::with_book(tx, &r.crew_id, |b, now| {
        b.close_entry(&Actor::Human, now, no, &p.note, None)
    })?;
    tx.note(Change::WorkChanged {
        crew_id: r.crew_id,
        id: p.id.clone(),
    });
    get(tx.conn(), &p.id)
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Notification {
    requirement_id: String,
    captain: String,
    stop_floor: i64,
}
/// Render and claim both call this; a stale hint never starts a model turn.
pub(crate) fn followup_note(
    conn: &Connection,
    row: &wakes::WakeRow,
) -> Result<crate::work::FollowupNote> {
    let id = row
        .source
        .strip_prefix(SOURCE)
        .ok_or_else(|| StoreError::Invalid("invalid requirement notification source".into()))?;
    let n: Notification = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| StoreError::Invalid("invalid requirement notification metadata".into()))?;
    if n.requirement_id != id || n.captain != row.session || n.stop_floor < 0 {
        return Err(
            StoreError::Invalid("requirement notification identity mismatch".into()).into(),
        );
    }
    let Some(r) = object_refs::get(
        conn,
        ObjectKind::Requirement,
        id.strip_prefix('#')
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|n| *n > 0 && id == format!("#{n}"))
            .ok_or_else(|| StoreError::Invalid("invalid requirement notification ID".into()))?,
    )?
    else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    if crews::is_archived(conn, &r.crew_id)?
        || crews::captain_of(conn, &r.crew_id)?.as_deref() != Some(row.session.as_str())
    {
        return Ok(crate::work::FollowupNote::Stale);
    }
    let Some(s) = crewd_store::sessions::get(conn, &row.session)? else {
        return Ok(crate::work::FollowupNote::Stale);
    };
    if s.crew_id != r.crew_id
        || (wakes::human_stop_floor(conn, &row.session)? != n.stop_floor || row.id <= n.stop_floor)
    {
        return Ok(crate::work::FollowupNote::Stale);
    }
    let detail = get(conn, id)?;
    if detail.todo.status != "unclaimed" || detail.closure.is_some() {
        return Ok(crate::work::FollowupNote::Stale);
    }
    Ok(crate::work::FollowupNote::Current(
        serde_json::json!({"newRequirement":id,"crewId":r.crew_id}).to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, store};
    fn input() -> TodoCreateParams {
        TodoCreateParams {
            channel_id: None,
            request_id: "d92c3929-67f8-4c67-8d17-1c7a1cd1f381".into(),
            crew_id: "crew-a".into(),
            text: "  immutable source\n正文原样  ".into(),
        }
    }
    fn rows(s: &crewd_store::Store) -> Vec<wakes::WakeRow> {
        wakes::list(s.conn(), Some("cap-a"), 100).unwrap()
    }
    fn lookup(p: &TodoCreateParams) -> TodoCreateLookupParams {
        TodoCreateLookupParams {
            request_id: p.request_id.clone(),
            crew_id: p.crew_id.clone(),
            text_sha256: original_text_sha256(&p.text),
            channel_id: p.channel_id.clone(),
        }
    }
    #[test]
    fn original_create_lookup_redacts_absence_and_checks_exact_utf8_and_reference() {
        let mut s = store();
        let mut p = input();
        p.text = "  Cafe\u{301}\n原文 ".into();
        let mut q = lookup(&p);
        assert_eq!(
            q.text_sha256,
            "ecb359a14d8b9313dcd747cf16d7195f1aeffedf799012ae353fcfdfb55f9599"
        );
        let absent = TodoCreateLookupResult::NotFound {
            request_id: p.request_id.clone(),
        };
        let before = s.conn().total_changes();
        assert_eq!(
            create_lookup(s.conn(), &Sender::Human, q.clone()).unwrap(),
            absent
        );
        assert_eq!(s.conn().total_changes(), before);

        let original = ok(&mut s, |tx| create(tx, &Sender::Human, p.clone()));
        let before = s.conn().total_changes();
        q.request_id = q.request_id.to_uppercase();
        assert_eq!(
            create_lookup(s.conn(), &Sender::Human, q.clone()).unwrap(),
            TodoCreateLookupResult::Committed {
                request_id: p.request_id.clone(),
                receipt: TodoCreateLookupReceipt {
                    crew_id: p.crew_id.clone(),
                    requirement_id: original.todo.requirement_id.clone().unwrap(),
                    created_at: original.todo.created_at.clone(),
                    text_sha256: q.text_sha256.clone(),
                    source_channel_id: None,
                    current_association: None,
                },
            },
        );
        let mut mismatch = q.clone();
        mismatch.text_sha256 = original_text_sha256("  Café\n原文 ");
        assert_eq!(
            create_lookup(s.conn(), &Sender::Human, mismatch).unwrap(),
            absent
        );
        let mut foreign = q.clone();
        foreign.crew_id = "crew-b".into();
        assert_eq!(
            create_lookup(s.conn(), &Sender::Human, foreign).unwrap(),
            absent
        );
        assert!(create_lookup(s.conn(), &Sender::Session("cap-a".into()), q.clone()).is_err());
        assert_eq!(s.conn().total_changes(), before);
        s.conn()
            .execute(
                "UPDATE object_refs SET core_id=NULL WHERE raw_todo_id=?1",
                [&original.todo.id],
            )
            .unwrap();
        let before = s.conn().total_changes();
        assert_eq!(create_lookup(s.conn(), &Sender::Human, q).unwrap(), absent);
        assert_eq!(s.conn().total_changes(), before);
    }

    #[test]
    fn original_create_lookup_refuses_copied_and_noncanonical_timestamps() {
        let mut s = store();
        let p = input();
        let original = ok(&mut s, |tx| create(tx, &Sender::Human, p.clone()));
        let original_ms = chrono::DateTime::parse_from_rfc3339(&original.todo.created_at)
            .unwrap()
            .timestamp_millis();
        let q = lookup(&p);
        for invalid_time in [
            "bad timestamp".into(),
            crewd_store::ts_from_ms(original_ms + 1),
        ] {
            s.conn()
                .execute(
                    "UPDATE todos SET created_at=?1 WHERE id=?2",
                    params![invalid_time, original.todo.id],
                )
                .unwrap();
            s.conn()
                .execute(
                    "UPDATE object_refs SET created_at=?1 WHERE raw_todo_id=?2",
                    params![invalid_time, original.todo.id],
                )
                .unwrap();
            let before = s.conn().total_changes();
            assert_eq!(
                create_lookup(s.conn(), &Sender::Human, q.clone()).unwrap(),
                TodoCreateLookupResult::NotFound {
                    request_id: p.request_id.clone()
                }
            );
            assert_eq!(s.conn().total_changes(), before);
        }
    }

    #[test]
    fn original_create_lookup_preserves_pre_channel_marker_and_workbook_clock_clamp() {
        let mut s = store();
        let p = input();
        let mut future = p.clone();
        future.request_id = uuid::Uuid::new_v4().to_string();
        crate::testkit::ok_at(&mut s, NOW + 60_000, |tx| {
            create(tx, &Sender::Human, future)
        });
        let original = ok(&mut s, |tx| create(tx, &Sender::Human, p.clone()));
        s.conn()
            .execute(
                "UPDATE todos SET extra=?1 WHERE id=?2",
                params![
                    serde_json::json!({"requirementRequestId":p.request_id}).to_string(),
                    original.todo.id,
                ],
            )
            .unwrap();
        let before = s.conn().total_changes();
        let mut q = lookup(&p);
        let TodoCreateLookupResult::Committed { receipt, .. } =
            create_lookup(s.conn(), &Sender::Human, q.clone()).unwrap()
        else {
            panic!("legacy original should be retained");
        };
        assert_eq!(receipt.created_at, crewd_store::ts_from_ms(NOW));
        assert_eq!(
            receipt.requirement_id,
            original.todo.requirement_id.unwrap()
        );
        assert!(receipt.source_channel_id.is_none() && receipt.current_association.is_none());
        q.channel_id = Some("any-channel".into());
        assert_eq!(
            create_lookup(s.conn(), &Sender::Human, q).unwrap(),
            TodoCreateLookupResult::NotFound {
                request_id: p.request_id
            }
        );
        assert_eq!(s.conn().total_changes(), before);
    }

    #[test]
    fn original_create_lookup_keeps_detached_and_archived_channel_state_and_redacts_purge() {
        use crate::channels::{ChannelPatch, NewChannel};
        use crewd_proto::{ChannelKind, ChannelWorkLinkParams};
        let mut s = store();
        let channel = ok(&mut s, |tx| {
            crate::channels::create(
                tx,
                &Sender::Human,
                NewChannel {
                    crew_id: "crew-b".into(),
                    kind: ChannelKind::Topic,
                    name: "original synthetic source".into(),
                    purpose: None,
                    members: vec![],
                },
            )
        })
        .id;
        let mut p = input();
        p.channel_id = Some(channel.clone());
        let original = ok(&mut s, |tx| create(tx, &Sender::Human, p.clone()));
        let id = original.todo.requirement_id.unwrap();
        let q = lookup(&p);
        let before = s.conn().total_changes();
        let TodoCreateLookupResult::Committed {
            receipt: linked, ..
        } = create_lookup(s.conn(), &Sender::Human, q.clone()).unwrap()
        else {
            panic!("missing original");
        };
        assert_eq!(linked.current_association, original.channel_association);
        assert!(linked.current_association.unwrap().linked);
        assert_eq!(s.conn().total_changes(), before);
        let detached = ok(&mut s, |tx| {
            crate::channel_work::link(
                tx,
                &Sender::Human,
                ChannelWorkLinkParams {
                    channel_id: channel.clone(),
                    expected_link_revision: original.channel_association.unwrap().link_revision,
                    object: ChannelWorkObject {
                        kind: ChannelWorkKind::Requirement,
                        id: id.clone(),
                    },
                },
                false,
            )
        });
        ok(&mut s, |tx| {
            crate::channels::update(
                tx,
                &Sender::Human,
                &channel,
                ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        });
        let before = s.conn().total_changes();
        let TodoCreateLookupResult::Committed { receipt, .. } =
            create_lookup(s.conn(), &Sender::Human, q.clone()).unwrap()
        else {
            panic!("archived original is readable");
        };
        assert_eq!(receipt.requirement_id, id);
        assert_eq!(receipt.source_channel_id.as_deref(), Some(channel.as_str()));
        let association = receipt.current_association.unwrap();
        assert!(!association.linked);
        assert_eq!(association.link_revision, detached.link_revision);
        assert_eq!(s.conn().total_changes(), before);
        let absent = TodoCreateLookupResult::NotFound {
            request_id: p.request_id,
        };
        let mut foreign = q.clone();
        foreign.channel_id = Some("unrelated-valid-id".into());
        assert_eq!(
            create_lookup(s.conn(), &Sender::Human, foreign).unwrap(),
            absent
        );
        ok(&mut s, |tx| crews::archive(tx, "crew-b"));
        ok(&mut s, |tx| crews::purge(tx, "crew-b"));
        let before = s.conn().total_changes();
        assert_eq!(create_lookup(s.conn(), &Sender::Human, q).unwrap(), absent);
        assert!(
            get(s.conn(), &id).is_ok(),
            "purging the original channel leaves the original requirement"
        );
        assert_eq!(s.conn().total_changes(), before);
    }

    #[test]
    fn same_request_retries_without_mutation_and_different_payload_is_conflict() {
        let mut s = store();
        let p = input();
        let first = ok(&mut s, |tx| create(tx, &Sender::Human, p.clone()));
        assert_eq!(first.todo.text, p.text);
        assert_eq!(first.todo.status, "unclaimed");
        let seq = s.conn().total_changes();
        let (_, changes) =
            crate::run(&mut s, NOW, |tx| create(tx, &Sender::Human, p.clone())).unwrap();
        assert!(changes.is_empty());
        assert_eq!(seq, s.conn().total_changes());
        let mut changed = p.clone();
        changed.text.push('x');
        assert!(
            matches!(err(&mut s,|tx|create(tx,&Sender::Human,changed)),CollabError::Conflict(r) if r.code==Code::TodoRequestConflict)
        );
        let mut blank = p.clone();
        blank.text = "  ".into();
        assert!(
            matches!(err(&mut s,|tx|create(tx,&Sender::Human,blank)),CollabError::Conflict(r) if r.code==Code::TodoRequestConflict)
        );
        assert_eq!(rows(&s).len(), 1);
        ok(&mut s, |tx| {
            close(
                tx,
                &Sender::Human,
                TodoCloseParams {
                    id: first.todo.requirement_id.clone().unwrap(),
                    expected_status: "unclaimed".into(),
                    note: "human ruling".into(),
                },
            )
        });
        let replay = ok(&mut s, |tx| create(tx, &Sender::Human, p));
        assert_eq!(replay.todo.status, "closed");
        assert_eq!(replay.closure.unwrap().note, "human ruling");
        assert_eq!(rows(&s).len(), 1);
    }
    #[test]
    fn a_legacy_row_at_the_request_key_is_not_adopted_and_uuid_spellings_share_identity() {
        let mut s = store();
        let p = input();
        let first = ok(&mut s, |tx| create(tx, &Sender::Human, p.clone()));
        let mut case = p.clone();
        case.request_id = case.request_id.to_uppercase();
        assert_eq!(ok(&mut s, |tx| create(tx, &Sender::Human, case)), first);
        s.conn()
            .execute("UPDATE todos SET extra=NULL WHERE id=?1", [&first.todo.id])
            .unwrap();
        let before = s.conn().total_changes();
        assert!(
            matches!(err(&mut s,|tx|create(tx,&Sender::Human,p)),CollabError::Conflict(r) if r.code==Code::TodoRequestConflict)
        );
        assert_eq!(s.conn().total_changes(), before);
        assert_eq!(rows(&s).len(), 1);
    }
    #[test]
    fn notification_guards_recheck_source_captain_freeze_and_human_stop() {
        for mode in ["claimed", "closed", "frozen", "captain", "stop", "spawn"] {
            let mut s = store();
            let req = ok(&mut s, |tx| create(tx, &Sender::Human, input()));
            let row = rows(&s).remove(0);
            assert!(matches!(
                followup_note(s.conn(), &row).unwrap(),
                crate::work::FollowupNote::Current(_)
            ));
            let id = req.todo.requirement_id.unwrap();
            match mode {
                "claimed" => {
                    ok(&mut s, |tx| {
                        crate::work::create(
                            tx,
                            &Sender::Human,
                            crewd_proto::WorkCreateParams {
                                crew_id: "crew-b".into(),
                                title: "plan".into(),
                                owner: Some("w-b1".into()),
                                sources: vec![id],
                                ..Default::default()
                            },
                        )
                    });
                }
                "closed" => {
                    ok(&mut s, |tx| {
                        close(
                            tx,
                            &Sender::Human,
                            TodoCloseParams {
                                id,
                                note: "ruling".into(),
                                expected_status: "unclaimed".into(),
                            },
                        )
                    });
                }
                "frozen" => {
                    ok(&mut s, |tx| crews::archive(tx, "crew-b"));
                    ok(&mut s, |tx| crews::archive(tx, "crew-a"));
                }
                "captain" => {
                    s.conn()
                        .execute(
                            "UPDATE crews SET captain_session_id='w-a1' WHERE id='crew-a'",
                            [],
                        )
                        .unwrap();
                }
                "stop" => {
                    ok(&mut s, |tx| wakes::human_stop(tx, "cap-a"));
                }
                "spawn" => {
                    ok(&mut s, |tx| {
                        sessions::set_desired(tx, "cap-a", crewd_proto::Desired::Running)
                    });
                }
                _ => unreachable!(),
            }
            let note = followup_note(s.conn(), &row).unwrap();
            if mode == "spawn" {
                assert!(matches!(note, crate::work::FollowupNote::Current(_)));
            } else {
                assert!(matches!(note, crate::work::FollowupNote::Stale), "{mode}");
            }
        }
    }
    #[test]
    fn no_captain_keeps_unclaimed_and_write_fault_rolls_back_all_authority() {
        let mut s = store();
        s.conn()
            .execute(
                "UPDATE crews SET captain_session_id=NULL WHERE id='crew-a'",
                [],
            )
            .unwrap();
        let r = ok(&mut s, |tx| create(tx, &Sender::Human, input()));
        assert_eq!(r.todo.status, "unclaimed");
        assert!(rows(&s).is_empty());
        let mut s = store();
        let before = asks::load_org(s.conn()).unwrap().state().clone();
        s.conn().execute_batch("CREATE TRIGGER reject_requirement_event BEFORE INSERT ON organization_work_events BEGIN SELECT RAISE(ABORT,'requirement event fault'); END;").unwrap();
        assert!(matches!(
            err(&mut s, |tx| create(tx, &Sender::Human, input())),
            CollabError::Store(_)
        ));
        assert_eq!(asks::load_org(s.conn()).unwrap().state(), &before);
        assert!(rows(&s).is_empty());
        assert_eq!(crate::testkit::count(&s, "SELECT COUNT(*) FROM todos"), 0);
        assert_eq!(
            crate::testkit::count(&s, "SELECT COUNT(*) FROM object_refs"),
            0
        );
    }
    #[test]
    fn only_human_writes_and_raw_text_never_changes_when_close_is_refused() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| create(
                tx,
                &Sender::Session("cap-a".into()),
                input()
            )),
            CollabError::Forbidden(_)
        ));
        let r = ok(&mut s, |tx| create(tx, &Sender::Human, input()));
        let id = r.todo.requirement_id.clone().unwrap();
        ok(&mut s, |tx| {
            crate::work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "active plan".into(),
                    owner: Some("w-a1".into()),
                    sources: vec![id.clone()],
                    ..Default::default()
                },
            )
        });
        assert!(
            matches!(err(&mut s,|tx|close(tx,&Sender::Human,TodoCloseParams{id:id.clone(),expected_status:"unclaimed".into(),note:"stale ruling".into()})),CollabError::Conflict(r) if r.code==Code::TodoStatusConflict)
        );
        assert!(matches!(
            err(&mut s, |tx| close(
                tx,
                &Sender::Human,
                TodoCloseParams {
                    id: id.clone(),
                    expected_status: "in_progress".into(),
                    note: "must not hide live work".into()
                }
            )),
            CollabError::Work(_)
        ));
        let current = get(s.conn(), &id).unwrap();
        assert_eq!(current.todo.text, r.todo.text);
        assert!(current.closure.is_none());
        assert!(get(s.conn(), "1").is_err());
    }
}
