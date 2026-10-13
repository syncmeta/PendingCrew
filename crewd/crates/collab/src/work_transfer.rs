//! Two-party offers use the existing organization book and real request-event identity.
use crate::refusal::{Code, Refusal};
use crate::wakes::{self, WakeKind, WakePriority, WakeRequest};
use crate::{CollabError, Result, Sender, Tx, asks, crews, sessions, work};
use crewd_proto::{WorkPlan, WorkTransferRequestParams, WorkTransferResolveParams};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_work::{EventKind, PendingTransfer, SessionId, Status, WorkId};
use rusqlite::Connection;
const PREFIX: &str = "work-transfer:";
fn invalid(s: &str) -> CollabError {
    StoreError::Invalid(s.into()).into()
}
/// Missing original request journal is unknown (legacy), never a synthetic identity.
pub(crate) fn request_seq(
    conn: &Connection,
    crew: &str,
    id: WorkId,
    pending: &PendingTransfer,
) -> Result<Option<u64>> {
    let Some(e) = crewd_store::work_organization::latest_transfer_request(conn, crew, id)? else {
        return Ok(None);
    };
    match &e.kind {
        EventKind::TransferRequested {
            id: target,
            to,
            note,
        } if *target == id
            && *to == pending.to
            && *note == pending.note
            && e.by.session() == Some(&pending.requested_by)
            && e.at == pending.at =>
        {
            Ok(Some(e.seq))
        }
        _ => Err(invalid(
            "pending transfer disagrees with genuine request journal",
        )),
    }
}
fn prepare(
    tx: &Tx<'_>,
    by: &Sender,
    id: &str,
    expected: &str,
) -> Result<(ObjectReference, WorkId, crewd_work::Actor)> {
    let r = object_refs::get(tx.conn(), ObjectKind::Plan, work::number(id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", id)))?;
    crews::refuse_archived(tx.conn(), &r.crew_id)?;
    let actor = work::actor(tx.conn(), by, &r.crew_id)?;
    let local = work::core_plan(tx.conn(), &r)?;
    let revision = expected
        .parse::<u64>()
        .ok()
        .filter(|v| expected == v.to_string())
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    let book = asks::load_org(tx.conn())?;
    work::check_revision(
        book.item(local)
            .ok_or_else(|| invalid("transfer plan lacks authority"))?,
        id,
        revision,
    )?;
    Ok((r, local, actor))
}
pub fn request(tx: &mut Tx<'_>, by: &Sender, p: WorkTransferRequestParams) -> Result<WorkPlan> {
    let (r, id, actor) = prepare(tx, by, &p.id, &p.expected_revision)?;
    let target = sessions::get(tx.conn(), &p.to)?;
    crews::refuse_archived(tx.conn(), &target.crew_id)?;
    asks::with_book(tx, &r.crew_id, |b, now| {
        b.request_transfer(&actor, now, id, SessionId::new(&p.to), &p.note)
    })?;
    let plan = work::get(tx.conn(), &p.id)?;
    let seq = plan
        .pending_transfer
        .as_ref()
        .and_then(|t| t.request_seq.clone())
        .ok_or_else(|| invalid("new transfer lacks genuine request event"))?;
    let n = Offer {
        work: p.id.clone(),
        request_seq: seq.clone(),
        target: p.to.clone(),
        stop_floor: wakes::human_stop_floor(tx.conn(), &p.to)?,
    };
    wakes::enqueue(
        tx,
        WakeRequest {
            session: p.to,
            kind: WakeKind::System,
            source: format!("{PREFIX}{}:{seq}", p.id),
            priority: WakePriority::System,
            not_before_ms: None,
            note: Some(serde_json::to_string(&n).map_err(StoreError::from)?),
            origin: None,
        },
    )?;
    work::changed(tx, by, &r.crew_id, &p.id)?;
    Ok(plan)
}
pub fn accept(tx: &mut Tx<'_>, by: &Sender, p: WorkTransferResolveParams) -> Result<WorkPlan> {
    let (r, id, actor) = prepare(tx, by, &p.id, &p.expected_revision)?;
    let book = asks::load_org(tx.conn())?;
    if let Some(t) = book.item(id).and_then(|i| i.pending_transfer.as_ref()) {
        let target = sessions::get(tx.conn(), t.to.as_str())?;
        crews::refuse_archived(tx.conn(), &target.crew_id)?;
    }
    asks::with_book(tx, &r.crew_id, |b, now| b.accept_transfer(&actor, now, id))?;
    work::changed(tx, by, &r.crew_id, &p.id)?;
    work::get(tx.conn(), &p.id)
}
pub fn withdraw(tx: &mut Tx<'_>, by: &Sender, p: WorkTransferResolveParams) -> Result<WorkPlan> {
    let (r, id, actor) = prepare(tx, by, &p.id, &p.expected_revision)?;
    asks::with_book(tx, &r.crew_id, |b, now| {
        b.withdraw_transfer(&actor, now, id)
    })?;
    work::changed(tx, by, &r.crew_id, &p.id)?;
    work::get(tx.conn(), &p.id)
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Offer {
    work: String,
    request_seq: String,
    target: String,
    stop_floor: i64,
}
pub(crate) fn followup_note(conn: &Connection, row: &wakes::WakeRow) -> Result<work::FollowupNote> {
    let n: Offer = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| invalid("transfer offer wake metadata is invalid"))?;
    let seq = n
        .request_seq
        .parse::<u64>()
        .ok()
        .filter(|s| *s > 0 && n.request_seq == s.to_string())
        .ok_or_else(|| invalid("transfer offer sequence is invalid"))?;
    if n.target != row.session
        || n.stop_floor < 0
        || row.priority != WakePriority::System
        || row.source != format!("{PREFIX}{}:{}", n.work, n.request_seq)
    {
        return Err(invalid("transfer offer wake identity is inconsistent"));
    }
    let Some(r) = object_refs::get(
        conn,
        ObjectKind::Plan,
        work::number(&n.work).map_err(|_| invalid("transfer offer public ID is invalid"))?,
    )?
    else {
        return Ok(work::FollowupNote::Stale);
    };
    if crews::is_archived(conn, &r.crew_id)?
        || wakes::automatic_work_paused(conn, &row.session)?
        || wakes::human_stop_floor(conn, &row.session)? != n.stop_floor
        || row.id <= n.stop_floor
    {
        return Ok(work::FollowupNote::Stale);
    }
    let Some(target) = crewd_store::sessions::get(conn, &row.session)? else {
        return Ok(work::FollowupNote::Stale);
    };
    if crews::is_archived(conn, &target.crew_id)? {
        return Ok(work::FollowupNote::Stale);
    }
    let book = asks::load_org(conn)?;
    let id = work::core_plan(conn, &r)?;
    let item = book
        .item(id)
        .ok_or_else(|| invalid("transfer offer plan lacks authority"))?;
    let Some(p) = &item.pending_transfer else {
        return Ok(work::FollowupNote::Stale);
    };
    if matches!(item.status, Status::Done | Status::Dropped)
        || p.to.as_str() != row.session
        || item.owner.as_ref() != Some(&p.requested_by)
    {
        return Ok(work::FollowupNote::Stale);
    }
    let current = request_seq(conn, &r.crew_id, id, p)?
        .ok_or_else(|| invalid("queued transfer has lost its genuine request journal"))?;
    if current != seq {
        return Ok(work::FollowupNote::Stale);
    }
    Ok(work::FollowupNote::Current(
        serde_json::json!({"offeredWork":n.work,"to":n.target,"requestSeq":n.request_seq})
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, store, v8_store};
    fn plan(s: &mut crewd_store::Store) -> WorkPlan {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "offer fixture".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        })
    }
    fn offer(s: &mut crewd_store::Store, p: &WorkPlan) -> WorkPlan {
        ok(s, |tx| {
            request(
                tx,
                &Sender::Session("w-a1".into()),
                WorkTransferRequestParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    to: "w-b1".into(),
                    note: "retain real offer".into(),
                },
            )
        })
    }
    #[test]
    fn genuine_legacy_pending_has_unknown_request_seq_without_counterfeit_event_or_wake() {
        let mut s = v8_store();
        let mut book = crewd_work::WorkBook::new(
            crewd_work::CrewId::new("crew-a"),
            crewd_work::Config::default(),
        );
        let id = book
            .create_item(
                &crewd_work::Actor::Human,
                crewd_work::Ts(NOW),
                crewd_work::NewItem::titled("legacy offer").owned_by("w-a1"),
            )
            .unwrap()
            .value;
        book.request_transfer(
            &crewd_work::Actor::worker("w-a1"),
            crewd_work::Ts(NOW),
            id,
            SessionId::new("w-a2"),
            "original legacy note",
        )
        .unwrap();
        let events = book.take_log();
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            tx.execute(
                "INSERT INTO work_books(crew_id,state,updated_at)VALUES('crew-a',?1,?2)",
                rusqlite::params![
                    serde_json::to_string(book.state()).unwrap(),
                    crewd_store::ts_from_ms(NOW)
                ],
            )
            .unwrap();
            crewd_store::work_events::append(&tx, "crew-a", &events).unwrap();
            object_refs::bind(
                &tx,
                ObjectKind::Plan,
                "crew-a",
                "work:W1",
                &crewd_store::ts_from_ms(NOW),
            )
            .unwrap();
            crewd_store::work_organization::migrate_in_tx(&tx, &crewd_store::ts_from_ms(NOW))
                .unwrap();
            tx.commit().unwrap();
        }
        let r =
            crewd_store::work_organization::reference_for_core(s.conn(), ObjectKind::Plan, "W1")
                .unwrap()
                .unwrap();
        let before = s.conn().total_changes();
        let p = work::get(s.conn(), &r.id()).unwrap();
        assert_eq!(p.pending_transfer.as_ref().unwrap().request_seq, None);
        assert_eq!(
            p.pending_transfer.as_ref().unwrap().note,
            "original legacy note"
        );
        assert_eq!(s.conn().total_changes(), before);
        assert_eq!(
            crate::testkit::count(&s, "SELECT COUNT(*) FROM organization_work_events"),
            0
        );
        assert_eq!(crate::testkit::count(&s, "SELECT COUNT(*) FROM wakes"), 0);
        let accepted = ok(&mut s, |tx| {
            accept(
                tx,
                &Sender::Session("w-a2".into()),
                WorkTransferResolveParams {
                    id: p.id,
                    expected_revision: p.revision,
                },
            )
        });
        assert_eq!(accepted.owner.as_deref(), Some("w-a2"));
        assert!(accepted.pending_transfer.is_none());
        assert_eq!(
            crate::testkit::count(&s, "SELECT COUNT(*) FROM organization_work_events"),
            1
        );
    }
    #[test]
    fn lost_new_offer_journal_is_an_error_and_never_an_empty_stale_guess() {
        let mut s = store();
        let p = plan(&mut s);
        let p = offer(&mut s, &p);
        let row = wakes::list(s.conn(), Some("w-b1"), 100).unwrap().remove(0);
        let seq = p.pending_transfer.unwrap().request_seq.unwrap();
        s.conn()
            .execute("DELETE FROM organization_work_events WHERE seq=?1", [seq])
            .unwrap();
        assert!(matches!(
            followup_note(s.conn(), &row),
            Err(CollabError::Store(_))
        ));
        let preserved = wakes::by_ids(s.conn(), &[row.id]).unwrap().remove(0);
        assert_eq!(preserved.state, wakes::WakeState::Pending);
        assert_eq!(preserved.attempts, 0);
    }
    #[test]
    fn later_human_stop_cancels_offer_hint_and_event_fault_preserves_original_owner() {
        let mut s = store();
        let p = plan(&mut s);
        let before = asks::load_org(s.conn()).unwrap().state().clone();
        s.conn().execute_batch("CREATE TRIGGER reject_offer BEFORE INSERT ON organization_work_events BEGIN SELECT RAISE(ABORT,'offer event fault'); END;").unwrap();
        assert!(matches!(
            err(&mut s, |tx| request(
                tx,
                &Sender::Session("w-a1".into()),
                WorkTransferRequestParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    to: "w-b1".into(),
                    note: "failure".into()
                }
            )),
            CollabError::Store(_)
        ));
        assert_eq!(asks::load_org(s.conn()).unwrap().state(), &before);
        assert!(wakes::list(s.conn(), Some("w-b1"), 100).unwrap().is_empty());
        s.conn().execute_batch("DROP TRIGGER reject_offer").unwrap();
        offer(&mut s, &p);
        let row = wakes::list(s.conn(), Some("w-b1"), 100).unwrap().remove(0);
        ok(&mut s, |tx| {
            sessions::set_desired(tx, "w-b1", crewd_proto::Desired::Running)
        });
        assert!(matches!(
            followup_note(s.conn(), &row).unwrap(),
            work::FollowupNote::Current(_)
        ));
        ok(&mut s, |tx| wakes::human_stop(tx, "w-b1"));
        assert!(matches!(
            followup_note(s.conn(), &row).unwrap(),
            work::FollowupNote::Stale
        ));
        let current = work::get(s.conn(), &p.id).unwrap();
        assert_eq!(current.owner.as_deref(), Some("w-a1"));
        assert!(current.pending_transfer.is_some());
    }
}
