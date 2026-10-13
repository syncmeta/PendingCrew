//! Human source facts plus derived organization state; no secondary lifecycle authority.
use crate::refusal::{Code, Refusal};
use crate::wakes::{self, WakeKind, WakePriority, WakeRequest};
use crate::{Change, CollabError, Result, Sender, Tx, asks, crews, sessions};
use crewd_proto::{
    Ledger, RequirementClosure, RequirementDetail, Todo, TodoCloseParams, TodoCreateParams,
};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_work::{Actor, EntryNo, EntryStatus};
use rusqlite::Connection;
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
fn reference(conn: &Connection, id: &str) -> Result<ObjectReference> {
    let n = id
        .strip_prefix('#')
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0 && id == format!("#{n}"))
        .ok_or_else(|| invalid(Code::TodoIdInvalid))?;
    object_refs::get(conn, ObjectKind::Requirement, n)?.ok_or_else(|| {
        CollabError::NotFound(Refusal::new(Code::TodoNotFound).arg("number", n.to_string()))
    })
}
fn status(s: EntryStatus) -> &'static str {
    match s {
        EntryStatus::Unclaimed => "unclaimed",
        EntryStatus::InProgress => "in_progress",
        EntryStatus::Done => "done",
        EntryStatus::NeedsRuling => "needs_ruling",
        EntryStatus::Closed => "closed",
    }
}
fn core(conn: &Connection, r: &ObjectReference) -> Result<EntryNo> {
    crewd_store::work_organization::core_for_reference(conn, ObjectKind::Requirement, r.number)?
        .ok_or_else(|| StoreError::Invalid("requirement lacks organization identity".into()))?
        .parse()
        .map_err(|_| {
            StoreError::Invalid("requirement has invalid organization identity".into()).into()
        })
}
pub fn get(conn: &Connection, id: &str) -> Result<RequirementDetail> {
    let r = reference(conn, id)?;
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
    let book = asks::load_org(conn)?;
    let no = core(conn, &r)?;
    let view = book
        .entry_view(no)
        .ok_or_else(|| StoreError::Invalid("requirement lacks authoritative entry".into()))?;
    todo.requirement_id = Some(r.id());
    todo.status = status(view.status).into();
    let closure = book.state().entries[&no]
        .closed
        .as_ref()
        .map(|c| RequirementClosure {
            note: c.note.clone(),
            at: crewd_store::ts_from_ms(c.at.0),
        });
    Ok(RequirementDetail { todo, closure })
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
        return get(tx.conn(), &id);
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
        Some(&serde_json::json!({"requirementRequestId":request})),
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
    if let Some(captain) = crews::captain_of(tx.conn(), &p.crew_id)? {
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
    get(tx.conn(), &id)
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
            request_id: "d92c3929-67f8-4c67-8d17-1c7a1cd1f381".into(),
            crew_id: "crew-a".into(),
            text: "  immutable source\n正文原样  ".into(),
        }
    }
    fn rows(s: &crewd_store::Store) -> Vec<wakes::WakeRow> {
        wakes::list(s.conn(), Some("cap-a"), 100).unwrap()
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
